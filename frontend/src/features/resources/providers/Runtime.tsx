/** A provider's independent keeper and runtime. Declared lifecycle decides attachment or unit controls;
 * every runtime has the same variables, secrets and permitted commands beside its actual harnesses.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { RotateCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { providerQuery, restartHarness } from '@/api/resources';
import { Facts } from '@/features/resources/Facts';
import { MachineLimits } from '@/features/resources/machines/Policy';
import { MachineLink } from '@/features/resources/machines/MachineList';
import type { HarnessState, Provider } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatDateTime, useNow } from '@/lib/time';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { adminSpec } from '@/ui/AdminAct';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { StatusDot } from '@/ui/StatusDot';
import { toast } from '@/ui/Toast';
import { linkClasses } from '@/ui/link';
import { PlugZap } from 'lucide-react';
import { startRuntime, stopRuntime } from '@/api/provider-runtime';
import { RuntimeConfiguration } from './RuntimeConfiguration';
import { providerRuntimeState } from './state';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { useState, type ReactNode } from 'react';
import { ResourceDetails } from '@/features/resources/DetailSections';
import { RuntimeChange } from '@/features/resources/TechnicalValue';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { UserChip } from '@/ui/UserChip';

const HARNESS_TONES = { online: 'success', starting: 'info', error: 'danger', offline: 'idle' } as const;
type ProfileReports = [HarnessState, ...HarnessState[]];

/** Reports on old machines remain diagnostic evidence; the ordinary list has one named profile. */
export function HarnessProfiles({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const profiles = new Map<string, ProfileReports>();
  for (const harness of provider.harnesses) {
    const rows = profiles.get(harness.agent.id);
    if (rows === undefined) { profiles.set(harness.agent.id, [harness]); continue; }
    const sameMachine = rows.findIndex((row) => row.machine.id === harness.machine.id);
    if (sameMachine < 0) rows.push(harness); else rows[sameMachine] = harness;
    profiles.set(harness.agent.id, rows);
  }
  return profiles.size === 0 ? <EmptyState compact title={t('providers.runtime.noHarnesses')} />
    : <ul aria-label={t('providers.runtime.harnesses')} className="flex flex-col divide-y divide-border rounded-lg border border-border bg-surface">
      {[...profiles.values()].map((reports) => <Harness key={reports[0].agent.id} provider={provider} reports={reports} />)}
    </ul>;
}

function Harness({ provider, reports }: { provider: Provider; reports: ProfileReports }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const priority = { online: 0, starting: 1, error: 2, offline: 3 } as const;
  const current = reports.find((report) => report.machine.id === provider.machine?.id);
  const reportedHere = provider.machine === null || current !== undefined;
  const harness = current
    ?? reports.reduce((best, report) => priority[report.status] < priority[best.status] || priority[report.status] === priority[best.status] && (report.last_active_at ?? '') > (best.last_active_at ?? '') ? report : best, reports[0]);
  const sessions = reports.reduce((count, report) => count + report.sessions_open, 0);
  const lastActive = reports.map((report) => report.last_active_at).filter((at): at is string => at !== null).sort().at(-1);
  const lastError = reports.flatMap((report) => report.last_error ? [{ ...report.last_error, machine: report.machine }] : []).sort((a, b) => b.at.localeCompare(a.at))[0];
  const restart = useMutation({
    mutationFn: () => restartHarness(provider.id, harness.agent.id),
    onSuccess: (updated) => {
      cache.setQueryData<Provider>(providerQuery(provider.id).queryKey, (held) => held !== undefined && held.revision > updated.revision ? held : updated);
      toast({ tone: 'success', title: t('providers.runtime.restarted', { name: harness.agent.name }) });
    },
  });
  return (
    <li className="flex flex-col gap-2 px-3 py-2">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <ResourceChip subject={{ kind: 'agent', agent: harness.agent }} asChild hint={false}><Link to="/resources/agents/$id" params={{ id: harness.agent.id }} className="font-medium text-text hover:underline">{harness.agent.name}</Link></ResourceChip>
        {reportedHere ? <StatusDot tone={HARNESS_TONES[harness.status]} label={t(`providers.runtime.harnessStatus.${harness.status}`)} showLabel /> : <span className="text-sm text-text-muted">{t('resourceDetails.harnessUnreported')}</span>}
        {reportedHere && harness.harness_version !== null && <span className="text-sm text-text-muted">{harness.harness_version}</span>}
        <span className="text-sm text-text-muted">{t('providers.runtime.sessions', { count: sessions })}</span>
        {lastActive !== undefined && (
          <span className="text-sm text-text-muted">
            {t('providers.runtime.lastActive')} <RelativeTime at={lastActive} />
          </span>
        )}
        <Button admin={adminSpec(provider, 'provider.manage')} size="sm" className="ml-auto" icon={<RotateCw aria-hidden />} pending={restart.isPending} onClick={() => restart.mutate()}>
          {t('providers.runtime.restart')}
        </Button>
      </div>
      {lastError !== undefined && <p className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 text-sm text-text-muted">
        <span>{t('providers.runtime.lastProgramError')}</span><RelativeTime at={lastError.at} />
        {reports.length > 1 && <MachineLink machine={lastError.machine} />}
        {lastError.detail !== null && <span className="min-w-0 whitespace-pre-wrap wrap-anywhere">{lastError.detail}</span>}
      </p>}
      {(!reportedHere || reports.length > 1 || reports.some((report) => report.status_detail !== null)) && <ResourceDetails><dl className="flex flex-col gap-1">{reports.map((report) => <div key={report.machine.id} className="flex flex-wrap items-center gap-2"><dt>{report.machine.name}</dt><dd>{t(`providers.runtime.harnessStatus.${report.status}`)} · {t('providers.runtime.sessions', { count: report.sessions_open })}{report.status_detail !== null && <p className="wrap-anywhere">{report.status_detail}</p>}</dd></div>)}</dl></ResourceDetails>}
      {restart.error !== null && <ErrorAlert error={restart.error} />}
    </li>
  );
}

function RuntimeDetails({ provider, configuration, children }: { provider: Provider; configuration: boolean; children: ReactNode }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const userId = useStore(client.session, (state) => state.userId);
  const { machine, runtime } = provider;
  const [stopping, setStopping] = useState(false);
  const cache=useQueryClient();
  const control = useMutation({ mutationFn: (action:'start'|'stop') => action === 'start' ? startRuntime(provider.id) : stopRuntime(provider.id, true), onSuccess: (updated) => { setStopping(false);cache.setQueryData(providerQuery(provider.id).queryKey,updated); } });

  if (provider.runtime_support?.lifecycle === 'attach' && machine !== null) {
    return (
      <div className="flex flex-col gap-6">
        <InlineAlert tone="info" title={t('providers.runtime.runsOn', { machine: machine.name, state: machine.online ? t('machines.state.online') : t('machines.state.offline') })}>
          {t('providers.runtime.lookedAfter')}
          {provider.owner.id === userId && (
            <>
              {' '}
              <Link to="/resources/machines/$id" params={{ id: machine.id }} className={linkClasses}>
                {t('providers.runtime.openMachine', { machine: machine.name })}
              </Link>
            </>
          )}
        </InlineAlert>
        <div className="flex flex-wrap gap-2"><Button admin={adminSpec(provider, 'provider.manage')} pending={control.isPending} disabled={!machine.online} onClick={()=>control.mutate('start')}>{t('providers.runtime.attach')}</Button><Button disabled={!provider.harnesses.some((harness)=>harness.machine.id===machine.id && harness.status==='online')} onClick={()=>setStopping(true)}>{t('providers.runtime.detach')}</Button></div>
        {control.error && <ErrorAlert error={control.error} />}
        {children}
        <MachineLimits machineId={machine.id} bounds="provider" id="provider-limits" title={t('providers.runtime.limits')} lead={t('providers.runtime.limitsLead')} />
        {configuration && <RuntimeConfiguration provider={provider} />}
        <ConfirmDialog admin={adminSpec(provider, 'provider.manage')} open={stopping} onOpenChange={setStopping} title={t('providers.runtime.detach')} confirmLabel={t('providers.runtime.detach')} pending={control.isPending} error={control.error} onConfirm={()=>control.mutate('stop')}>{t('providerRuntime.stopHelp',{count:provider.running_threads})}</ConfirmDialog>
      </div>
    );
  }

  if (runtime === null) {
    return (
      <div className="flex flex-col gap-4">
      <EmptyState
        icon={<PlugZap aria-hidden />}
        title={t('providers.runtime.none')}
      />
      {configuration && <RuntimeConfiguration key={provider.id} provider={provider} />}
      {(provider.runtime_support?.lifecycle==='unit' || machine===null && provider.runtime_support?.lifecycle!=='attach') && <Button admin={adminSpec(provider, 'provider.manage')} pending={control.isPending} onClick={() => control.mutate('start')}>{t('providerRuntime.start')}</Button>}
      {control.error && <ErrorAlert error={control.error} />}
      {children}
      </div>
    );
  }

  const context = { locale, timeZone };
  return (
    <div className="flex flex-col gap-6">
      <Section id="provider-runtime" title={t('providers.runtime.title')}>
        <Facts
          label={t('providers.runtime.title')}
          rows={[
            { name: t('providers.runtime.state'), value: providerRuntimeState(runtime.state,t) },
            { name: t('providers.runtime.host'), value: runtime.host === null ? t('providers.runtime.noHost') : <ResourceChip subject={{ kind: 'host', host: runtime.host }} /> },
            { name: t('providers.runtime.template'), value: <span className="flex flex-wrap items-center gap-1"><ResourceChip subject={{ kind: 'template', template: runtime.template }} hint={false} />{runtime.template.owner !== null && <UserChip user={runtime.template.owner} />}</span> },
            ...(runtime.idle_stop_at === null ? [] : [{ name: t('providers.runtime.idleStop'), value: formatDateTime(runtime.idle_stop_at, context, now) }]),
            ...(runtime.renew_at === null ? [] : [{ name: t('providers.runtime.renew'), value: formatDateTime(runtime.renew_at, context, now) }]),
          ]}
        />
        {runtime.update_required && <Badge tone="warning">{t('providers.phrase.updateRequired')}</Badge>}
        {runtime.updates.length > 0 && (
          <ul aria-label={t('providers.runtime.updates')} className="flex flex-col gap-1 text-base text-text">
            {runtime.updates.map((update) => (
              <li key={update.name}><RuntimeChange change={update} /></li>
            ))}
          </ul>
        )}
        <div className="flex flex-wrap gap-2"><Button admin={adminSpec(provider, 'provider.manage')} pending={control.isPending} disabled={runtime.state !== 'stopped' && runtime.state !== 'failed'} onClick={() => control.mutate('start')}>{t('providerRuntime.start')}</Button><Button disabled={runtime.state !== 'running'} onClick={() => setStopping(true)}>{t('providerRuntime.stop')}</Button></div>
        {control.error && <ErrorAlert error={control.error} />}
      </Section>
      {children}
      {configuration && <RuntimeConfiguration key={provider.id} provider={provider} />}
      <ConfirmDialog admin={adminSpec(provider, 'provider.manage')} open={stopping} onOpenChange={setStopping} title={t('providerRuntime.stop')} confirmLabel={t('providerRuntime.stop')} pending={control.isPending} error={control.error} onConfirm={() => control.mutate('stop')}>{t('providerRuntime.stopHelp', { count:provider.running_threads })}</ConfirmDialog>
    </div>
  );
}

export function ProviderRuntime({provider,configuration=true}:{provider:Provider;configuration?:boolean}) {
  const {t}=useTranslation();
  return <div className="flex flex-col gap-4">
    <InlineAlert tone="info">{provider.credential_keeper===undefined || provider.credential_keeper===null ? t('providers.runtime.keeperWaiting') : t('providers.runtime.keeperPlace',{machine:provider.credential_keeper.name})}</InlineAlert>
    <RuntimeDetails provider={provider} configuration={configuration}>
    {(provider.harness_kind !== null || provider.harnesses.length > 0) && <Section id="provider-harnesses" title={t('providers.runtime.harnesses')} lead={t('providers.runtime.harnessesLead')}>
      <HarnessProfiles provider={provider} />
    </Section>}
    </RuntimeDetails>
  </div>;
}
