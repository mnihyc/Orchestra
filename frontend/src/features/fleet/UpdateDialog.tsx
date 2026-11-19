/** Update selected environments and provider runtimes with one reviewed plan and one running-turn choice. */
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { planEnvironmentUpdates, planNodeUpdates, planProviderUpdates, startEnvironmentUpdates, startNodeUpdates, startProviderUpdates } from '@/api/changes';
import { cancelJob, isOver, jobQuery, planGone } from '@/api/forks';
import { ApiError } from '@/api/client';
import { harnessUsageQuery } from '@/api/harnesses';
import { Frame } from '@/features/change/Frame';
import { JobProgress, committed } from '@/features/change/Progress';
import { useRefreshWhenOver } from '@/features/change/refresh';
import { TargetBuilds } from '@/features/change/TargetBuilds';
import { UpdateNotify } from '@/features/change/UpdateNotify';
import { RuntimeChange } from '@/features/resources/TechnicalValue';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import type { ActiveTurn, EnvironmentRef, Job, PendingUpdate, ProviderRef, RunningHarnessBuild } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { StateLine } from '@/ui/StateLine';
import { updateUnits } from './updateWords';

export interface UpdateTargets { environments: readonly string[]; providers?: readonly string[]; node?: string }
interface PlanRow { id: string; environment?: EnvironmentRef; provider?: ProviderRef; updates: PendingUpdate[]; programs?: RunningHarnessBuild[] | null; running: number; kept: string[]; lost: string[]; reasons: string[] }
const CHANGE_COLUMN = 'change';

export function RunningTurnChoice({ activeTurn, minutes, running, disabled = false, onChoice, onMinutes }: { activeTurn: ActiveTurn; minutes: number; running: number; disabled?: boolean; onChoice: (choice: ActiveTurn) => void; onMinutes: (minutes: number) => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return <div className="flex flex-wrap items-end gap-2">
    <SegmentedControl label={t('change.options.turn')} value={activeTurn} onValueChange={onChoice} segments={[
      { value: 'wait', name: t('changeCompact.turn.wait'), disabled }, { value: 'interrupt', name: t('changeCompact.turn.interrupt'), disabled },
    ]} />
    {activeTurn === 'wait' && <Input label={t('change.options.minutes')} type="number" min={0} step={1} value={String(minutes)} disabled={disabled} fieldClassName="w-(--field-narrow)" onChange={(event) => { const value = Number(event.target.value); if (Number.isSafeInteger(value) && value >= 0) onMinutes(value); }} />}
    <span className="text-sm text-text-muted" role="status">{t(activeTurn === 'interrupt' ? 'programVersions.stops' : 'programVersions.waits', { count: running, number: formatNumber(running, locale) })}</span>
  </div>;
}

export function UpdateDialog({ targets, onClose }: { targets: UpdateTargets; onClose: () => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const { locale } = useFormatContext();
  const key = useActKey();
  const [choice, setChoice] = useState<ActiveTurn | null>(null);
  const [wait, setWait] = useState<number | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [notify, setNotify] = useState(true);
  const [cancelling, setCancelling] = useState<Job | null>(null);
  const allEnvironments = [...new Set(targets.environments)].sort();
  const allProviders = [...new Set(targets.providers ?? [])].sort();
  const [pool, setPool] = useState<'environment' | 'provider'>(allEnvironments.length > 0 ? 'environment' : 'provider');
  const environmentIds = pool === 'environment' ? allEnvironments : [];
  const providerIds = pool === 'provider' ? allProviders : [];
  const plan = useQuery({ queryKey: ['batch-unit-update', environmentIds, providerIds, targets.node], queryFn: async ({ signal }) => {
    const [environments, providers] = await Promise.all([
      environmentIds.length === 0 ? null : targets.node === undefined ? planEnvironmentUpdates(environmentIds, signal) : planNodeUpdates(targets.node, environmentIds, signal),
      providerIds.length === 0 ? null : planProviderUpdates(providerIds, signal),
    ]);
    return { environments, providers };
  }, retry: false, staleTime: Number.POSITIVE_INFINITY, gcTime: 0, enabled: jobs.length === 0 });
  const values = plan.data;
  const defaults = values?.environments?.options ?? values?.providers?.options;
  const activeTurn = choice ?? defaults?.active_turn ?? 'wait';
  const minutes = wait ?? defaults?.wait_minutes ?? 10;
  const rows: PlanRow[] = [
    ...(values?.environments?.plans ?? []).map((row) => ({ id: row.environment.id, environment: row.environment, updates: row.updates, programs: row.target_programs, running: row.threads.filter((thread) => thread.turn_running).length, kept: row.kept, lost: row.lost, reasons: row.blockers.map((blocker) => blocker.message) })),
    ...(values?.providers?.plans ?? []).map((row) => ({ id: row.provider.id, provider: row.provider, updates: row.updates, programs: row.target_programs, running: row.threads.filter((thread) => thread.turn_running).length, kept: row.kept, lost: row.lost, reasons: row.blockers.map((blocker) => blocker.message) })),
    ...(values?.environments?.skipped ?? []).map((row) => ({ id: row.environment.id, environment: row.environment, updates: [], running: 0, kept: [], lost: [], reasons: [t(`programVersions.skip.${row.reason}`)] })),
    ...(values?.providers?.skipped ?? []).map((row) => ({ id: row.provider.id, provider: row.provider, updates: [], running: 0, kept: [], lost: [], reasons: [t(`programVersions.skip.${row.reason}`)] })),
  ];
  const executable = (values?.environments?.executable ?? 0) + (values?.providers?.executable ?? 0);
  const plannedIds = new Set(rows.map((row) => row.id));
  const unavailable = values === undefined ? 0 : [...environmentIds,...providerIds].filter((id) => !plannedIds.has(id)).length;
  const run = useMutation({ mutationFn: async () => {
    if (values === undefined) return;
    const options = { active_turn: activeTurn, wait_minutes: minutes };
    if (values.environments !== null && values.environments.executable > 0) {
      const body = { preflight_id: values.environments.preflight_id, options };
      const batch = await (targets.node === undefined ? startEnvironmentUpdates(body, key({body,notify}), notify) : startNodeUpdates(targets.node, body, key({body,notify}), notify));
      setJobs(batch.jobs.map((entry) => entry.job));
    } else if (values.providers !== null && values.providers.executable > 0) {
      const body = { preflight_id: values.providers.preflight_id, options };
      const batch = await startProviderUpdates(body, key({body,notify}), notify);
      setJobs(batch.jobs.map((entry) => entry.job));
    }
  }, onError: (error) => { if (planGone(error) || error instanceof ApiError && error.is('conflict.stale_revision')) void plan.refetch(); }, onSuccess: () => { void client.invalidateQueries({ queryKey: harnessUsageQuery.queryKey, exact: true }); } });
  const progress = useQueries({ queries: jobs.map((job) => jobQuery(job.id)) });
  const currentJobs = jobs.map((job, index) => progress[index]?.data ?? job);
  const over = currentJobs.length > 0 && currentJobs.every((job) => isOver(job.state));
  const cancel = useMutation({ mutationFn: (id: string) => cancelJob(id), onSuccess: () => setCancelling(null) });
  const finished = currentJobs.filter((job) => isOver(job.state)).map((job) => job.id).sort().join(',');
  useRefreshWhenOver(finished !== '', finished, currentJobs.filter((job) => isOver(job.state)).flatMap((job) => job.environment_id === null ? [] : [job.environment_id]));
  const audience = values?.environments?.notify ?? values?.providers?.notify;
  const runningCount = rows.filter((row) => row.reasons.length === 0).reduce((sum, row) => sum + row.running, 0);
  const switchPool = (next: 'environment' | 'provider') => {
    if (run.isPending || jobs.length > 0 && !over) return;
    setPool(next); setJobs([]); run.reset(); cancel.reset(); setCancelling(null);
  };
  const changes = (row: PlanRow) => <div className="flex min-w-0 flex-col gap-1">{row.updates.map((update) => <div key={update.name} className="text-sm wrap-anywhere"><RuntimeChange change={update} /></div>)}<TargetBuilds updates={row.updates} programs={row.programs} />{row.reasons.map((reason) => <span key={reason} className="text-sm wrap-anywhere text-text-muted">{reason}</span>)}{row.kept.length + row.lost.length > 0 && <div className="flex flex-col gap-1 text-sm text-text-muted"><span>{row.kept.join(' · ')}</span><span className="text-warning-text">{row.lost.join(' · ')}</span></div>}</div>;
  const columns: Column<PlanRow>[] = [
    { id: 'unit', header: t('programVersions.unit'), cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1">{row.environment !== undefined ? <ResourceChip subject={{ kind: 'environment', environment: row.environment }} /> : row.provider !== undefined ? <ResourceChip subject={{ kind: 'provider', provider: row.provider }} /> : null}{row.running > 0 && <span className="text-sm text-text-muted">{t('programVersions.runningTurns', { count: row.running, number: formatNumber(row.running, locale) })}</span>}{!shown.has(CHANGE_COLUMN) && changes(row)}</div> },
    { id: CHANGE_COLUMN, header: t('programVersions.changes'), from: 'sm', grow: true, cell: changes },
  ];
  return <Frame open onClose={onClose} title={t('programVersions.updateTitle')} at={jobs.length === 0 ? run.isPending ? 'confirm' : 'plan' : over ? 'done' : 'progress'} failed={over && currentJobs.some((job) => job.state === 'failed' || job.state === 'cancelled')} dismissible={!run.isPending}
    footer={<>{jobs.length === 0 && <UpdateNotify audience={audience} checked={notify} onChange={setNotify} disabled={run.isPending} />}<Button disabled={run.isPending} onClick={onClose}>{t(jobs.length === 0 ? 'common.cancel' : over ? 'common.close' : 'change.progress.hide')}</Button>{jobs.length === 0 && <Button variant="primary" disabled={plan.isPending || plan.isFetching || plan.isError || executable === 0} pending={run.isPending} onClick={() => run.mutate()}>{t('programVersions.updateCount', { units: updateUnits(values?.environments?.executable ?? 0, values?.providers?.executable ?? 0, locale, t) })}</Button>}</>}>
    {allEnvironments.length > 0 && allProviders.length > 0 && <SegmentedControl label={t('programVersions.updateScope')} value={pool} onValueChange={switchPool} segments={[
      {value:'environment',name:t('programVersions.environmentCount',{count:allEnvironments.length,number:formatNumber(allEnvironments.length,locale)}),description:t('programVersions.environmentPool'),disabled:run.isPending || jobs.length > 0 && !over},
      {value:'provider',name:t('programVersions.runtimeCount',{count:allProviders.length,number:formatNumber(allProviders.length,locale)}),description:t('programVersions.providerPool'),disabled:run.isPending || jobs.length > 0 && !over},
    ]} />}
    {jobs.length === 0 ? <>{plan.isPending ? <SkeletonRows rows={4} /> : plan.isError ? <ErrorAlert error={plan.error} message={plan.error instanceof ApiError && plan.error.is('request.invalid') ? t('programVersions.planRefused', { reason: plan.error.problem.detail }) : undefined} action={<Button onClick={() => void plan.refetch()}>{t('common.tryAgain')}</Button>} /> : <>
      <RunningTurnChoice activeTurn={activeTurn} minutes={minutes} running={runningCount} onChoice={setChoice} onMinutes={setWait} disabled={run.isPending} />
      {unavailable > 0 && <StateLine compact tone="warning" message={t(pool === 'environment' ? 'programVersions.unavailableEnvironments' : 'programVersions.unavailableRuntimes',{count:unavailable,number:formatNumber(unavailable,locale)})} />}
      <Table label={t('programVersions.updateTitle')} columns={columns} rows={rows} rowKey={(row) => row.id} />
    </>}</> : currentJobs.map((job) => {
      const row = rows.find((row) => row.id === (job.environment_id ?? job.provider_id));
      const name = row?.environment?.name ?? row?.provider?.name ?? t('programVersions.updateTitle');
      return <div key={job.id} className="flex flex-col gap-2">
        {row?.environment !== undefined ? <ResourceChip subject={{ kind: 'environment', environment: row.environment }} /> : row?.provider !== undefined ? <ResourceChip subject={{ kind: 'provider', provider: row.provider }} /> : null}
        <JobProgress job={job} heading={name} reconnecting={false} />
        {!isOver(job.state) && <Button size="sm" variant="ghost" disabled={committed(job) || job.progress.activity?.stopping === true} pending={cancel.isPending && cancel.variables === job.id} onClick={() => setCancelling(job)}>{t(job.progress.activity?.stopping === true ? 'liveWork.stopping' : 'change.progress.cancel')}</Button>}
      </div>;
    })}
    {run.error !== null && <ErrorAlert error={run.error} message={run.error instanceof ApiError && run.error.is('request.invalid') ? t('programVersions.planRefused',{reason:run.error.problem.detail}) : undefined} />}
    <ConfirmDialog open={cancelling !== null} onOpenChange={(open) => !open && !cancel.isPending && setCancelling(null)} title={t('change.cancel.title')} confirmLabel={t('change.progress.cancel')} danger pending={cancel.isPending} error={cancel.error} onConfirm={() => cancelling !== null && cancel.mutate(cancelling.id)}>{t('change.cancel.body', { subject: rows.find((row) => row.id === (cancelling?.environment_id ?? cancelling?.provider_id))?.environment?.name ?? rows.find((row) => row.id === cancelling?.provider_id)?.provider?.name ?? t('programVersions.updateTitle') })}</ConfirmDialog>
  </Frame>;
}
