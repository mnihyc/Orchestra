/** Quiet source-labelled observations beside the existing profile fields. */
import { useTranslation } from 'react-i18next';
import type { HarnessKindInfo, Machine, MachineProgram } from '@/generated/api-types';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { CurrentDetailPage } from '@/features/resources/CurrentDetailPage';
import { Facts } from '@/features/resources/Facts';
import { arrangementName } from '@/features/resources/providers/Arrangements';
import { StatusDot } from '@/ui/StatusDot';
import { profileDetection } from './profileDetection';

export function AgentDetection({ kind, machine, catalogue, registered }: {
  kind: string; machine: Machine | undefined; catalogue: HarnessKindInfo | undefined; registered?: MachineProgram;
}) {
  const { t } = useTranslation();
  const report = profileDetection(machine?.facts, kind, catalogue, registered);
  const accounts = (machine?.providers ?? []).filter((provider) => provider.kind === 'subscription' && provider.harness_kind === kind);
  const source = (value: 'machine' | 'catalogue') => <span className="text-sm text-text-muted">{value === 'machine' ? t('agentDetection.fromMachine') : t('agentDetection.fromCatalogue')}</span>;
  const arrangements = (values: readonly string[] | null) => values === null ? t('agentDetection.notReported') : values.length === 0 ? t('agentDetection.none')
    : <CurrentDetailPage count={values.length}>{(from, to) => <span>{values.slice(from, to).map((value) => arrangementName(t, value)).join(t('startFlow.separator'))}</span>}</CurrentDetailPage>;
  return <Facts compact label={t('agentDetection.title')} rows={[
    { name: t('agents.overview.version'), value: <span className="inline-flex min-w-0 flex-wrap gap-x-2">
      {report.version ?? (machine === undefined ? t('agentDetection.chooseMachine') : t('agents.overview.versionUnknown'))}
      {report.version !== null && source('machine')}
    </span> },
    { name: t('agentDetection.accounts'), value: accounts.length === 0 ? (machine === undefined ? t('agentDetection.chooseMachine') : t('agentDetection.accountsUnknown'))
      : <CurrentDetailPage count={accounts.length}>{(from, to) => <ul className="flex min-w-0 flex-col gap-1">{accounts.slice(from, to).map((provider) => <li key={provider.id} className="flex min-w-0 flex-wrap items-center gap-2">
        <ResourceChip subject={{ kind: 'provider', provider }} hint={false} />
        <StatusDot tone={provider.status === 'ready' ? 'success' : provider.status === 'needs_setup' || provider.status === 'exhausted' ? 'warning' : provider.status === 'error' ? 'danger' : 'idle'} label={t(`providers.status.${provider.status}`)} showLabel />
      </li>)}</ul>}</CurrentDetailPage> },
    { name: t('agentDetection.programSupports'), value: <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">{arrangements(report.arrangements)}{report.source !== null && source(report.source)}</div> },
    ...(machine === undefined ? [] : [{ name: t('agentDetection.machineAllows'), value: <div className="flex min-w-0 flex-wrap items-baseline gap-x-2">{arrangements(report.machineArrangements)}{report.machineArrangements !== null && source('machine')}</div> }]),
    { name: t('agentDetection.arrangement'), value: <span className="inline-flex min-w-0 flex-wrap gap-x-2">{t('agentDetection.automatic')}<span className="text-sm text-text-muted">{t('agentDetection.threadDefaults')}</span></span> },
  ]} />;
}
