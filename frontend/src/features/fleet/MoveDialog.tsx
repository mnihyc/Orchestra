/** Review several environments and recover each independently with one destination and one progress bar. */
import { Hint } from '@/features/help/Hint';
import { keepPreviousData, useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { ResourceChip, resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { moveRemovesSoftware } from '@/features/resources/isolation/runtimes';
import { useTranslation } from 'react-i18next';
import { cancelJob, forkKeys, isOver, jobQuery } from '@/api/forks';
import { planRecovery, recoverableQuery, recoveryBatchQuery, startRecovery } from '@/api/loss';
import { hostsQuery } from '@/api/resources';
import { BatchDestination } from '@/features/change/BatchDestination';
import { NotifyPeople } from '@/features/notifications/NotifyPeople';
import { reasonWords } from '@/features/change/Result';
import { JobActivityLine } from '@/features/change/Progress';
import { Measurements, MeasurementReason } from '@/features/change/Measurements';
import { blockerWords } from '@/features/change/plan';
import type { EnvironmentBatch, EnvironmentRecoverPreflightBody, EnvironmentRecoveryRow, ForkEnvironment, Job, LossTarget, RecoveryAction, RecoveryChoice } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { useDebounced } from '@/lib/debounce';
import { formatBytes, formatNumber } from '@/lib/format';
import { useActKey } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';
import { Select } from '@/ui/Select';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { StatusDot } from '@/ui/StatusDot';
import { Table } from '@/ui/Table';

type Source = { ids: readonly string[] } | { target: LossTarget };

export function MoveDialog({ source, batch = null, open, onClose }: { source: Source; batch?: string | null; open: boolean; onClose: () => void }) {
  const { t } = useTranslation();
  const target = 'target' in source ? source.target : { type: 'environment' as const, id: '' };
  const [viewBatch, setViewBatch] = useState(batch);
  const ongoing = useQuery({ ...recoveryBatchQuery(viewBatch ?? ''), enabled: viewBatch !== null });
  const candidates = useQuery({ ...recoverableQuery(target), enabled: 'target' in source && viewBatch === null });
  const ids = 'ids' in source ? source.ids : (candidates.data?.environments ?? []).map((row) => row.id);
  const [started, setStarted] = useState<EnvironmentBatch | null>(null);
  const [retryIds, setRetryIds] = useState<readonly string[] | null>(null);
  const progress = started ?? (viewBatch === null ? null : ongoing.data ?? null);
  return <Dialog open={open} onOpenChange={(next) => !next && onClose()} title={t('loss.recoverTitle')} size="xl">
    {progress === null && ids.length === 0 && (candidates.data?.others ?? 0) > 0 && <Others count={candidates.data?.others ?? 0} />}
    {progress !== null ? <RecoveryProgress batch={progress} onClose={onClose} onRetry={(failed) => { setRetryIds(failed); setStarted(null); setViewBatch(null); }} /> : viewBatch !== null ? ongoing.isError ? <ErrorState compact error={ongoing.error} onRetry={() => void ongoing.refetch()} /> : <SkeletonRows rows={3} /> : 'target' in source && candidates.isPending ? <SkeletonRows rows={3} /> : 'target' in source && candidates.isError ? <ErrorState compact error={candidates.error} retrying={candidates.isRefetching} onRetry={() => void candidates.refetch()} /> : ids.length === 0 ? <EmptyState title={t('loss.noEnvironments')} /> : <RecoveryForm ids={retryIds ?? ids} others={candidates.data?.others ?? 0} onStarted={setStarted} onClose={onClose} />}
  </Dialog>;
}

function RecoveryForm({ ids, others, onStarted, onClose }: { ids: readonly string[]; others: number; onStarted: (batch: EnvironmentBatch) => void; onClose: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const key = useActKey();
  const client = useQueryClient();
  const hosts = useQuery(hostsQuery(null));
  const [host, setHost] = useState('');
  const [notify, setNotify] = useState<boolean | undefined>();
  const [choices, setChoices] = useState<RecoveryChoice[]>([]);
  const [destination, setDestination] = useState<ForkEnvironment | undefined>();
  const body: EnvironmentRecoverPreflightBody = { environment_ids: [...ids], choices, ...(host === '' ? {} : { host_id: host }), ...(destination === undefined ? {} : { environment: destination }) };
  const asked = JSON.stringify(body);
  const settled = useDebounced(asked);
  const plan = useQuery({ queryKey: ['recovery-plan', settled], queryFn: ({ signal }) => planRecovery(JSON.parse(settled) as EnvironmentRecoverPreflightBody, signal), staleTime: Number.POSITIVE_INFINITY, gcTime: 0, retry: false, placeholderData: keepPreviousData });
  const start = useMutation({ mutationFn: () => {
    if (plan.data === undefined) throw new Error('the recovery plan is unavailable');
    return startRecovery({ preflight_id: plan.data.preflight_id, notify: notify ?? plan.data.notification.default_on }, key({ plan: plan.data.preflight_id }));
  }, onSuccess: (batch) => { for (const row of batch.jobs) client.setQueryData(forkKeys.job(row.job.id), row.job); onStarted(batch); }, onError: () => { void plan.refetch(); } });
  const change = (environment_id: string, patch: Partial<RecoveryChoice>, clearHost = false) => setChoices((before) => {
    const next = { ...before.find((choice) => choice.environment_id === environment_id), environment_id, ...patch };
    if (clearHost) delete next.host_id;
    return [...before.filter((choice) => choice.environment_id !== environment_id), next];
  });
  const hostOptions = [{ value: 'automatic', name: t('changeCompact.automatic') }, ...(hosts.data ?? []).filter((candidate) => candidate.machine.online && candidate.lost_at === undefined && candidate.schedulable).map((candidate) => ({ value: candidate.id, name: candidate.name, ...resourceChoice({ kind: 'host', host: candidate }) }))];
  const carrying = (row: EnvironmentRecoveryRow) => {
    if (row.blockers.some((blocker) => blocker.code === 'measurement_pending')) return <MeasurementReason waiting planning={stale} fallback={null} />;
    const files = row.files ?? row.move_plan?.files;
    const coverage = !row.listing_available ? t('loss.listingUnavailable') : files === undefined ? row.switch_plan === null ? '' : t('loss.threads', { count: row.switch_plan.plans.length, number: formatNumber(row.switch_plan.plans.length, locale) }) : t('loss.fileCoverage', { copied: formatNumber(files.file_count - files.missing.count, locale), total: formatNumber(files.file_count, locale), missing: formatNumber(files.missing.count, locale) });
    if (!row.restored) return coverage;
    const restored = t('loss.restored', { size: row.restored_bytes === null ? '–' : formatBytes(row.restored_bytes, locale) });
    return !row.listing_available || (files !== undefined && (files.missing.count > 0 || files.bytes_to_download > 0)) ? `${restored} · ${coverage}` : restored;
  };
  const actions = (row: EnvironmentRecoveryRow) => <Select label={t('loss.actionFor', { name: row.environment.name })} hideLabel size="sm" value={row.action} onValueChange={(value) => change(row.environment.id, { action: value as RecoveryAction })} options={[
    ...(row.environment.status === 'lost' ? [{ value: 'recreate', name: t('loss.recreate') }] : [{ value: 'move', name: t('change.menu.move') }]), { value: 'switch_threads', name: t('loss.switchThreads') },
  ]} />;
  const destinationFor = (row: EnvironmentRecoveryRow) => row.action === 'switch_threads' ? null : <Select label={t('loss.hostFor', { name: row.environment.name })} hideLabel size="sm" value={choices.find((choice) => choice.environment_id === row.environment.id)?.host_id ?? 'as-above'} onValueChange={(value) => change(row.environment.id, value === 'as-above' ? {} : { host_id: value }, value === 'as-above')} options={[{ value: 'as-above', name: row.host?.host.name ?? t('loss.asAbove') }, ...hostOptions.slice(1)]} />;
  const stale = plan.isFetching || plan.isPlaceholderData || settled !== asked;
  const waiting = plan.data?.rows.some((row) => row.blockers.some((blocker) => blocker.code === 'measurement_pending')) ?? false;
  const countPlans = plan.data?.rows.flatMap((row) => [{ files: row.files }, ...(row.move_plan === null ? [] : [{ ...row.move_plan, homeFallback: true }]), ...(row.switch_plan?.plans ?? [])]) ?? [];
  return <Measurements plan={{ plans: countPlans }} active onReady={() => void plan.refetch()}><div className="flex min-w-0 flex-col gap-3">
    <Select help={<Hint popover id="loss-recover" />} label={t('loss.destinationHost')} value={host === '' ? 'automatic' : host} onValueChange={(next) => setHost(next === 'automatic' ? '' : next)} options={hostOptions} />
    {(plan.data?.rows.some((row) => row.action === 'switch_threads') ?? false) && <BatchDestination value={destination} onChange={setDestination} sources={ids} allowSame={false} />}
    {plan.isError ? <ErrorState compact error={plan.error} retrying={plan.isRefetching} onRetry={() => void plan.refetch()} /> : plan.isPending ? <SkeletonRows rows={ids.length} /> : <div aria-busy={stale} className={stale ? 'opacity-60' : undefined}><Table label={t('loss.recoveryPlans')} rows={plan.data.rows} rowKey={(row) => row.environment.id} columns={[
      { id: 'name', header: t('loss.environment'), cell: (row, shown) => <div className="flex min-w-0 flex-col gap-1"><span className="flex items-center gap-2"><ResourceChip subject={{ kind: 'environment', environment: { id: row.environment.id, name: row.environment.name, status: row.environment.status } }} asChild hint={false}><a href={`/resources/environments/${row.environment.id}`}>{row.environment.name}</a></ResourceChip><EnvironmentDot status={row.environment.status} showLabel /></span>{!shown.has('carries') && <span className="text-sm text-text-muted">{carrying(row)}</span>}{!shown.has('action') && actions(row)}{!shown.has('host') && destinationFor(row)}{row.move_plan !== null && moveRemovesSoftware(row.move_plan) && <span className="text-sm text-warning-text">{t('isolation.replaced')}</span>}{row.blockers.filter((blocker) => blocker.code !== 'measurement_pending').map((blocker) => <span key={blocker.code} className="text-sm text-warning-text">{blockerWords(blocker, null, t).text}</span>)}</div> },
      { id: 'action', header: t('loss.action'), from: 'sm', cell: actions },
      { id: 'carries', header: t('loss.carries'), from: 'md', cell: (row) => <span className="text-sm text-text-muted">{carrying(row)}</span> },
      { id: 'host', header: t('loss.destinationHost'), from: 'lg', cell: destinationFor },
    ]} /></div>}
    {others + (plan.data?.others ?? 0) > 0 && <Others count={others + (plan.data?.others ?? 0)} />}
    {(plan.data?.admin_grants ?? []).length > 0 && <StateLine compact tone="warning" message={t('loss.adminGrant')} />}
    {start.error !== null && <ErrorAlert error={start.error} />}
    <div className="flex flex-wrap items-center justify-end gap-2"><div className="me-auto min-w-0">{waiting ? <span role="status" data-change-reason><MeasurementReason waiting planning={stale} fallback={null} /></span> : <NotifyPeople plan={plan.data?.notification} value={notify} onChange={setNotify} disabled={start.isPending || stale} />}</div><div className="ms-auto flex shrink-0 items-center gap-2"><Button onClick={onClose} disabled={start.isPending}>{t('common.cancel')}</Button><Button variant="primary" pending={start.isPending} disabled={start.isPending || plan.data === undefined || stale || plan.data.executable === 0} onClick={() => start.mutate()}>{t('loss.confirm')}</Button></div></div>
  </div></Measurements>;
}

function RecoveryProgress({ batch, onClose, onRetry }: { batch: EnvironmentBatch; onClose: () => void; onRetry: (ids: string[]) => void }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const queries = useQueries({ queries: batch.jobs.map((row) => jobQuery(row.job.id)) });
  const rows = batch.jobs.map((row, index) => ({ ...row, job: queries[index]?.data ?? row.job }));
  const over = rows.every((row) => isOver(row.job.state));
  const failed = rows.filter((row) => row.job.state === 'failed' || row.job.state === 'lost');
  const percent = rows.length === 0 ? 0 : Math.floor(rows.reduce((sum, row) => sum + row.job.progress.percent, 0) / rows.length);
  const [cancelling,setCancelling]=useState(false);
  const stop = useMutation({ mutationFn: async () => { for (const row of rows.filter((row) => !isOver(row.job.state))) { const job = await cancelJob(row.job.id); client.setQueryData(forkKeys.job(job.id), job); } } });
  return <div className="flex flex-col gap-3" data-recovery-progress>
    <Meter look="progress" value={percent} label={t(over ? 'loss.progressOver' : 'loss.progressRunning')} />
    <Table label={t('loss.recoveryPlans')} rows={rows} rowKey={(row) => row.job.id} columns={[
      { id: 'name', header: t('loss.environment'), cell: (row) => <ResourceChip subject={{ kind: 'environment', environment: { id: row.environment.id, name: row.environment.name, status: row.environment.status } }} asChild hint={false}><a href={`/resources/environments/${row.environment.id}`}>{row.environment.name}</a></ResourceChip> },
      { id: 'state', header: t('loss.action'), cell: (row) => <div className="flex flex-col gap-1"><RecoveryState job={row.job} />{row.job.error !== null && <StateLine compact tone="danger" message={reasonWords(row.job, t, 10)} />}</div> },
    ]} />
    {stop.error !== null && <ErrorAlert error={stop.error} />}
    <div className="flex flex-wrap justify-end gap-2">{over && failed.length > 0 && <Button onClick={() => onRetry([...new Set(failed.map((row) => row.environment.id))])}>{t('change.batch.retry', { count: failed.length })}</Button>}{!over && <Button variant="danger" pending={stop.isPending} onClick={() => setCancelling(true)}>{t('change.progress.cancel')}</Button>}<Button onClick={onClose}>{t(over ? 'common.close' : 'change.progress.hide')}</Button></div>
    <ConfirmDialog open={cancelling} onOpenChange={setCancelling} title={t('loss.cancelTitle')} confirmLabel={t('change.progress.cancel')} danger pending={stop.isPending} error={stop.error} onConfirm={() => { stop.mutate(undefined,{onSuccess:()=>setCancelling(false)}); }}>{t('loss.cancelEffect')}</ConfirmDialog>
  </div>;
}

function RecoveryState({ job }: { job: Job }) {
  const { t } = useTranslation();
  if (job.state === 'running' || job.state === 'queued') return <div className="flex min-w-0 items-center gap-2"><StatusDot tone={job.state === 'running' ? 'info' : 'idle'} label={t(`source.job.${job.state}`)} /><JobActivityLine job={job} /></div>;
  return <StatusDot tone={job.state === 'succeeded' ? 'success' : job.state === 'failed' ? 'danger' : job.state === 'lost' ? 'warning' : 'off'} label={t(`source.job.${job.state}`)} showLabel />;
}

function Others({ count }: { count: number }) { const { t } = useTranslation();const {locale}=useFormatContext();return <p className="text-sm text-text-muted">{t('loss.others',{count,number:formatNumber(count,locale)})}</p>; }
