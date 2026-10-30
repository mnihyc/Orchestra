/** Compact resource detail bodies and their permitted quick actions, mounted only by an open inspection card. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { useState } from 'react';

import { environmentKey, environmentQuery, startEnvironment, stopEnvironment } from '@/api/environments';
import { hostQuery, providerQuery } from '@/api/resources';
import { openChange } from '@/features/change/target';
import { Hint } from '@/features/help/Hint';
import { effortDisplayName, modelDisplayName } from '@/features/models/names';
import { Facts, type Fact } from '@/features/resources/Facts';
import { ProviderUsageCard } from '@/features/resources/providers/ProviderUsageCard';
import { choiceObservation, speedText, timeText } from '@/features/usage/performance';
import type { CapStatus, Environment, Host, Provider } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber, formatPercent } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { formatSpan, useNow } from '@/lib/time';
import { adminSpec } from '@/ui/AdminAct';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Meter } from '@/ui/Meter';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { type ResourceSubject, DISK_BYTES_PER_GB, fullEnvironment, fullHost, fullProvider, MEMORY_BYTES_PER_MB, modelCaps, resourceAddress, resourceName } from './facts';
import ExtraResourceCard from './ExtraResourceCard';
import { ResourceHint, ResourceName } from './ResourceFacts';
import { useResourceSample } from './live';

function ReadState({ error, retry }: { error: unknown; retry: () => void }) {
  const { t } = useTranslation();
  return error === null || error === undefined ? <SkeletonRows rows={3} />
    : <ErrorAlert error={error} action={<Button size="sm" onClick={retry}>{t('common.tryAgain')}</Button>} />;
}

function Limits({ caps }: { caps: readonly CapStatus[] }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (caps.length === 0) return null;
  return <ul className="flex flex-col gap-2" aria-label={t('resourceFacts.limits')}>
    {caps.map((status) => {
      const amount = status.cap.amount === null ? null : Number(status.cap.amount);
      const used = Number(status.used);
      const label = t(`resourceFacts.meter.${status.cap.meter}`);
      return <li key={status.cap.id} className="flex min-w-0 flex-col gap-1">
        <span className="flex flex-wrap justify-between gap-x-3 text-sm"><span>{label}</span><StatusDot tone={status.state === 'ok' ? 'success' : 'warning'} label={t(`resourceFacts.capState.${status.state}`)} showLabel /></span>
        {amount !== null && amount > 0 && Number.isFinite(used) && <Meter neutral showActual look="compact" label={label} value={used / amount * 100} />}
        <span className="flex flex-wrap gap-x-3 text-sm text-text-muted">
          <span>{t('resourceFacts.usedOf', { used: formatNumber(used, locale), amount: amount === null ? t('providerPlan.unlimited') : formatNumber(amount, locale) })}</span>
          {status.waiting > 0 && <span>{t('resourceFacts.waitingCount', { count: status.waiting, value: formatNumber(status.waiting, locale) })}</span>}
          {status.period_ends_at !== null && <span>{t('providerDetails.resets')}{' '}<RelativeTime at={status.period_ends_at} /></span>}
          {status.cap.models.length > 0 && <span>{status.cap.models.join(', ')}</span>}
          {status.cap.tiers.length > 0 && <span>{status.cap.tiers.join(', ')}</span>}
        </span>
      </li>;
    })}
  </ul>;
}

function ProviderDetails({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const allowed = holds(provider.access, 'provider.view');
  const usage = allowed ? provider.choice_usage : undefined;
  const rows = usage?.performance ?? [];
  const fastest = rows.filter((row) => choiceObservation([row], row.model, row.effort, row.tier, row.program) !== null)
    .reduce<(typeof rows)[number] | undefined>((picked, row) => picked === undefined || row.performance.requests > picked.performance.requests ? row : picked, undefined);
  return <>
    <Facts compact rows={[
      { name: t('resourceFacts.status'), value: <StatusDot tone={provider.status === 'ready' ? 'success' : provider.status === 'disabled' ? 'off' : 'warning'} label={t(`providers.status.${provider.status}`)} showLabel /> },
      ...(allowed ? [{ name: t('resourceFacts.runningTurns'), value: formatNumber(provider.running_threads, locale) }] : []),
      ...(fastest === undefined ? [] : [{ name: t('resourceFacts.sevenDaySpeed'), value: <span>{speedText(fastest.performance.speed_milli === null ? null : fastest.performance.speed_milli / 1000, locale, t)}{' · '}{modelDisplayName(provider.models.find((model) => model.model_name === fastest.model)?.display_name)}</span> }]),
    ]} />
    <ProviderUsageCard compact allWindows provider={provider} />
    <Limits caps={usage?.caps ?? []} />
    {usage !== undefined && usage !== null && <RelativeTime at={usage.observed_at} className="text-xs text-text-subtle" />}
    <a href={`/resources/providers/${provider.id}`} className="self-start text-sm text-accent-text hover:underline">{t('resourceFacts.openProvider')}</a>
  </>;
}

function ModelDetails({ subject }: { subject: Extract<ResourceSubject, { kind: 'model' }> }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const model = subject.model;
  const parameters = model.parameters;
  const row = choiceObservation(subject.usage?.performance ?? [], model.model_name, subject.effort ?? parameters.default_effort ?? null, null, null);
  const rate = parameters.rate;
  return <>
    <Facts compact rows={[
      ...(parameters.context_window === null || parameters.context_window === undefined ? [] : [{ name: t('resourceFacts.context'), value: formatNumber(parameters.context_window, format.locale) }]),
      ...((parameters.efforts?.length ?? 0) === 0 ? [] : [{ name: t('resourceFacts.efforts'), value: parameters.efforts?.map((effort) => effortDisplayName(effort, t)).join(', ') }]),
      ...(subject.speed === null || subject.speed === undefined ? row === null ? [] : [{ name: t('resourceFacts.sevenDaySpeed'), value: speedText(row.performance.speed_milli === null ? null : row.performance.speed_milli / 1000, format.locale, t) }, { name: t('resourceFacts.firstOutput'), value: timeText(row.performance.first_token_ms, format.locale, t) }] : [{ name: t('resourceFacts.sevenDaySpeed'), value: subject.speed }]),
      ...(rate === null || rate === undefined ? [] : [{ name: t('resourceFacts.price'), value: t('resourceFacts.priceValue', { input: rate.input_per_mtok, output: rate.output_per_mtok }) }]),
      ...(subject.impact === undefined ? [] : [{ name: t('resourceFacts.applies'), value: subject.impact }]),
    ]} />
    <Limits caps={modelCaps(subject)} />
    <a href={`/resources/providers/${subject.providerId}?tab=models`} className="self-start text-sm text-accent-text hover:underline">{t('resourceFacts.openModels')}</a>
  </>;
}

function HostDetails({ host, subject }: { host: Host; subject: Extract<ResourceSubject, { kind: 'host' }> }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const place = subject.place;
  const declared = place?.declared ?? host.declared;
  const capacity = place?.capacity ?? host.capacity;
  const manage = holds(host.access, 'host.manage');
  return <>
    <Facts compact rows={[
      { name: t('resourceFacts.status'), value: <span className="inline-flex flex-wrap items-center gap-2"><StatusDot tone={host.machine.online ? 'success' : 'warning'} label={t(host.machine.online ? 'machines.state.online' : 'machines.state.offline')} showLabel />{!host.machine.online && host.offline_since !== undefined && host.offline_since !== null && <RelativeTime at={host.offline_since} />}</span> },
      { name: t('resourceFacts.runningEnvironments'), value: formatNumber(place?.environments.running ?? host.running.environment, format.locale) },
      ...(host.stopped_environments === null || host.stopped_environments === undefined ? [] : [{ name: t('resourceFacts.stoppedEnvironments'), value: formatNumber(host.stopped_environments, format.locale) }]),
      { name: t('resourceFacts.mine'), value: formatNumber(host.mine, format.locale) },
      { name: t('resourceFacts.schedulable'), value: t(host.schedulable ? 'resourceFacts.yes' : 'resourceFacts.no') },
      ...(host.usage?.memory_used_mb === undefined || host.usage.memory_used_mb === null ? [] : [{ name: t('resourceFacts.memoryNow'), value: formatBytes(host.usage.memory_used_mb * MEMORY_BYTES_PER_MB, format.locale) }]),
      ...(host.usage?.cpu_percent_milli === undefined || host.usage.cpu_percent_milli === null ? [] : [{ name: t('resourceFacts.cpu'), value: formatPercent(host.usage.cpu_percent_milli / 100_000, format.locale) }]),
      ...(host.usage === null || host.usage === undefined ? [] : [{ name: t('resourceFacts.observed'), value: <RelativeTime at={host.usage.observed_at} /> }]),
      ...(place?.storages ?? host.storages).map((storage) => ({ name: storage.name, value: t('resourceFacts.free', { size: formatBytes(storage.free_gb * DISK_BYTES_PER_GB, format.locale) }) })),
    ]} />
    {capacity.memory_mb > 0 && <Meter neutral showActual value={declared.memory_mb / capacity.memory_mb * 100} label={t('resourceFacts.declaredMemory', { used: formatBytes(declared.memory_mb * MEMORY_BYTES_PER_MB, format.locale), total: formatBytes(capacity.memory_mb * MEMORY_BYTES_PER_MB, format.locale) })} />}
    <div className="flex flex-wrap items-center justify-between gap-2">
      <a href={`/resources/hosts/${host.id}`} className="text-sm text-accent-text hover:underline">{t('resourceFacts.openHost')}</a>
      {manage && <Button size="sm" admin={adminSpec(host, 'host.manage')} onClick={() => openChange({ kind: 'host', id: host.id, name: host.name }, { kind: 'recycle' })}>{t('change.menu.recycle')}</Button>}
    </div>
  </>;
}

function EnvironmentActions({ environment }: { environment: Environment }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [stopping, setStopping] = useState(false);
  const start = useMutation({ mutationFn: () => startEnvironment(environment.id), onSuccess: () => client.invalidateQueries({ queryKey: environmentKey(environment.id), exact: true }) });
  const stop = useMutation({ mutationFn: () => stopEnvironment(environment.id, false), onSuccess: (next) => { client.setQueryData(environmentKey(environment.id), next); setStopping(false); } });
  const may = holds(environment.access, 'environment.manage') && environment.machine === null && environment.job === null;
  const guarded = environment.network.required_mode === 'restricted' && environment.network.mode === 'open';
  return <>
    <div className="flex flex-wrap items-center justify-between gap-2">
      <a href={`/resources/environments/${environment.id}`} className="text-sm text-accent-text hover:underline">{t('resourceFacts.openEnvironment')}</a>
      {may && !guarded && (environment.status === 'stopped' || environment.status === 'none') && <Button size="sm" admin={adminSpec(environment, 'environment.manage')} pending={start.isPending} onClick={() => start.mutate()}>{t('resources.act.start')}</Button>}
      {may && environment.status === 'running' && <Button size="sm" onClick={() => setStopping(true)}>{t('resources.act.stop')}</Button>}
    </div>
    {(start.error ?? stop.error) !== null && <ErrorAlert error={start.error ?? stop.error} />}
    <ConfirmDialog admin={adminSpec(environment, 'environment.manage')} open={stopping} onOpenChange={setStopping} title={t('resourceFacts.stopTitle', { name: environment.name })} confirmLabel={t('resourceFacts.stopTitle', { name: environment.name })} danger pending={stop.isPending} error={stop.error ?? undefined} onConfirm={() => stop.mutate()}>
      {t('resourceFacts.stopEffect', { name: environment.name })}
    </ConfirmDialog>
  </>;
}

function EnvironmentDetails({ environment, subject }: { environment: Environment; subject: Extract<ResourceSubject, { kind: 'environment' }> }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useNow();
  const measured = useResourceSample(environment.id, environment.status === 'running' && holds(environment.access, 'terminal.view'));
  const sample = subject.sample ?? measured;
  const activity = environment.activity;
  const rows: Fact[] = [
    { name: t('resourceFacts.status'), value: <EnvironmentDot status={environment.status} showLabel /> },
    ...(environment.host === null ? [] : [{ name: t('resourceFacts.host'), value: <ResourceName subject={{ kind: 'host', host: environment.host }} /> }]),
    { name: t('resourceFacts.threads'), value: activity === null || activity === undefined ? '–' : t('resourceFacts.threadCounts', { running: formatNumber(activity.running_threads, format.locale), idle: formatNumber(activity.idle_threads, format.locale), blocked: formatNumber(activity.blocked_threads, format.locale) }) },
    { name: t('resourceFacts.runningTurns'), value: activity === null || activity === undefined ? '–' : formatNumber(activity.active_turns, format.locale) },
    { name: t('resourceFacts.shells'), value: activity === null || activity === undefined ? '–' : formatNumber(activity.open_shells, format.locale) },
    { name: t('resourceFacts.processes'), value: sample === null ? '–' : formatNumber(sample.pids, format.locale) },
    { name: t('resourceFacts.cpu'), value: sample === null ? '–' : formatPercent(sample.cpu_percent / 100, format.locale) },
    { name: t('resourceFacts.memoryNow'), value: sample === null ? '–' : formatBytes(sample.memory_bytes, format.locale) },
    { name: t('resourceFacts.workspaceSize'), value: sample?.disk_used_bytes === null || sample?.disk_used_bytes === undefined ? '–' : formatBytes(sample.disk_used_bytes, format.locale) },
    ...(sample === null ? [] : [{ name: t('resourceFacts.observed'), value: <RelativeTime at={sample.at} /> }]),
    ...(environment.last_activity_at === null ? [] : [{ name: t('resourceFacts.idle'), value: formatSpan(Math.max(0, now - Date.parse(environment.last_activity_at)), format) }]),
    ...(environment.idle_stop_at === null ? [] : [{ name: t('resourceFacts.idleStop'), value: <RelativeTime at={environment.idle_stop_at} /> }]),
  ];
  return <><Facts compact rows={rows} /><EnvironmentActions environment={environment} /></>;
}

export default function ResourceCard({ subject }: { subject: ResourceSubject }) {
  const providerId = subject.kind === 'provider' ? subject.provider.id : subject.kind === 'model' ? subject.providerId : '';
  const hostId = subject.kind === 'host' ? subject.host.id : '';
  const environmentId = subject.kind === 'environment' ? subject.environment.id : '';
  // A passing card becoming kept shares its in-flight read with the new layer.
  const modelNeedsDetails = subject.kind === 'model' && providerId !== '' && (Object.keys(subject.model.parameters).length === 0 || subject.usage === undefined);
  const provider = useQuery({ ...providerQuery(providerId, { keepReading: true }), enabled: subject.kind === 'provider' && !fullProvider(subject.provider) || modelNeedsDetails });
  const host = useQuery({ ...hostQuery(hostId, { keepReading: true }), enabled: subject.kind === 'host' && !fullHost(subject.host) });
  const environment = useQuery({ ...environmentQuery(environmentId, { keepReading: true }), enabled: subject.kind === 'environment' && !fullEnvironment(subject.environment) });
  let content;
  let resolved = subject;
  if (subject.kind === 'model') {
    const model = provider.data?.models.find((model) => model.model_name === subject.model.model_name);
    const current = { ...subject, model: model ?? subject.model, usage: subject.usage ?? provider.data?.choice_usage };
    resolved = current;
    content = <><ModelDetails subject={current} />{modelNeedsDetails && provider.isError && <ReadState error={provider.error} retry={() => void provider.refetch()} />}</>;
  }
  else if (subject.kind === 'provider') {
    const row = fullProvider(subject.provider) ? subject.provider : provider.data;
    if (row !== undefined) resolved = { ...subject, provider: row };
    content = row === undefined ? <ReadState error={provider.isError ? provider.error : null} retry={() => void provider.refetch()} /> : <ProviderDetails provider={row} />;
  } else if (subject.kind === 'host') {
    const row = fullHost(subject.host) ? subject.host : host.data;
    if (row !== undefined) resolved = { ...subject, host: row };
    content = row === undefined ? <ReadState error={host.isError ? host.error : null} retry={() => void host.refetch()} /> : <HostDetails host={row} subject={subject} />;
  } else if (subject.kind === 'environment') {
    const row = fullEnvironment(subject.environment) ? subject.environment : environment.data;
    if (row !== undefined) resolved = { ...subject, environment: row };
    content = row === undefined ? <ReadState error={environment.isError ? environment.error : null} retry={() => void environment.refetch()} /> : <EnvironmentDetails environment={row} subject={subject} />;
  } else content = <ExtraResourceCard subject={subject} />;
  const named = resolved.kind === 'provider' ? resolved.provider : resolved.kind === 'host' ? resolved.host
    : resolved.kind === 'environment' ? resolved.environment : resolved.kind === 'mount' ? resolved.mount
      : resolved.kind === 'mcp_server' ? resolved.server : resolved.kind === 'template' ? resolved.template
        : resolved.kind === 'agent' ? resolved.agent : resolved.kind === 'skill' ? resolved.skill : null;
  const home = named !== null && 'home' in named ? named.home : null;
  return <div className="flex w-[min(20rem,calc(100vw-2rem))] max-w-full flex-col gap-3 p-3" data-resource-card={subject.kind}>
    <div className="flex min-w-0 flex-col gap-1"><div className="flex min-w-0 items-center gap-1"><h3 className="min-w-0 text-base font-semibold wrap-anywhere"><a href={resourceAddress(resolved)} className="hover:underline">{resourceName(resolved)}</a></h3><Hint id="resource-facts" popover /></div><ResourceHint subject={resolved} /></div>
    {home?.name !== undefined && home.name !== null && <p className="text-sm text-text-muted wrap-anywhere">{home.name}</p>}
    {content}
  </div>;
}
