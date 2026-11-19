/** Kept volumes, current restore/purge guards and the actual durable job, with no optimistic restoration. */
import { Cleanup } from '@/features/storage/Cleanup';
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Boxes, Eye, RotateCcw, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { environmentQuery } from '@/api/environments';
import { keptEnvironmentsQuery, restoreEnvironment, retainedEnvironmentQuery, retentionJobQuery } from '@/api/trace';
import { newActKey } from '@/lib/requests';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { Badge } from '@/ui/Badge';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { InspectionBanner } from './Inspection';
import { inspectionHref } from './objects';
import { retainedJobWords } from './jobWords';
import { JobActivityLine } from '@/features/change/Progress';
import { useResourceHeading } from '@/features/resources/trail';
import type { DeletedEnvironment } from '@/generated/api-types';

/** HTTP progress fills the gaps between durable job updates without spinning on each render. */
type RemovalEntry = Omit<DeletedEnvironment, 'removal'> & { removal?: DeletedEnvironment['removal'] | null };
export function RemovalFacts({ entry }: { entry: RemovalEntry }) {
  const { t } = useTranslation(); const format = useFormatContext(); const facts = entry.removal ?? null;
  if (facts === null) return <EmptyState compact title={t('trace.noContent')} icon={<Eye aria-hidden />} />;
  const day = facts.purge_at === null ? t('trace.unknown') : new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(facts.purge_at));
  return <div className="flex min-w-0 flex-col gap-2"><div className="flex flex-wrap items-center gap-2"><span className="font-medium wrap-anywhere">{entry.environment.name}</span><Badge tone={facts.phase === 'purged' ? 'neutral' : 'warning'}>{t(`trace.phases.${facts.phase}`)}</Badge></div><dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm"><dt className="text-text-muted">{t('trace.size')}</dt><dd>{facts.bytes === null ? t('trace.unknown') : formatBytes(facts.bytes, format.locale)}</dd><dt className="text-text-muted">{t('trace.home')}</dt><dd className="wrap-anywhere">{entry.home.redacted ? t('trace.unknown') : entry.home.name ?? t('trace.unknown')}</dd><dt className="text-text-muted">{t('trace.host')}</dt><dd className="wrap-anywhere">{entry.host?.name ?? t('trace.unknown')}</dd><dt className="text-text-muted">{t('trace.storage')}</dt><dd className="wrap-anywhere">{entry.storage ?? t('trace.unknown')}</dd></dl><p className="text-sm text-text-muted">{facts.phase === 'purged' ? t('trace.gone') : t('trace.until', { day })}</p>{facts.deleted_by !== null && <p className="text-sm text-text-muted">{t('places.deleted.by.person', { name: facts.deleted_by.display_name })}</p>}{facts.reason !== null && <p className="text-sm wrap-anywhere text-warning-text">{facts.reason}</p>}</div>;
}
export function JobProgress({ id, environmentId }: { id: string; environmentId: string }) {
  const { t } = useTranslation(); const cache = useQueryClient();
  const job = useQuery(retentionJobQuery(id));
  const restored = job.data?.kind === 'environment_restore' && job.data.state === 'succeeded';
  const live = useQuery({ ...environmentQuery(environmentId), enabled: restored, staleTime: 0 });
  useEffect(() => { if (job.data !== undefined && !['queued', 'running'].includes(job.data.state)) { void cache.invalidateQueries({ queryKey: ['trace-deleted'] }); void cache.invalidateQueries({ queryKey: ['host'] }); void cache.invalidateQueries({ queryKey: ['hosts'] }); void cache.invalidateQueries({ queryKey: ['trace'] }); } }, [job.data?.state, cache]); // eslint-disable-line react-hooks/exhaustive-deps
  if (job.isPending) return <SkeletonRows rows={1} />;
  if (job.isError) return <ErrorState compact error={job.error} onRetry={() => void job.refetch()} />;
  const state = t(`source.job.${job.data.state}`);
  return <div className="flex flex-col gap-2 rounded-lg border border-border bg-bg-subtle p-3" role="status">
    {job.data.error !== null ? <ErrorAlert error={job.data.error} /> : <StateLine compact message={state} />}
    {(job.data.state === 'queued' || job.data.state === 'running') && <Meter look="progress" value={job.data.progress.percent} label={state} />}
    {(job.data.state === 'queued' || job.data.state === 'running') && <JobActivityLine job={job.data} />}
    {live.data?.status === 'stopped' && <StateLine compact message={t('trace.restoreStopped')} action={<Button size="sm" action="open" asChild><a href={`/resources/environments/${environmentId}`}>{t('trace.openEnvironment')}</a></Button>} />}
    {live.isError && <ErrorAlert error={live.error} />}
  </div>;
}
function PurgeDialog({ entry, onClose, onAccepted, onRefused }: { entry: DeletedEnvironment; onClose: () => void; onAccepted: (id: string) => void; onRefused: () => void }) {
  return <Cleanup request={{scope:'me',resource:{kind:'environment',id:entry.environment.id},items:[`environment:${entry.environment.id}`]}} onClose={onClose} onAccepted={(job)=>onAccepted(job.id)} onFinished={onRefused} onRefused={onRefused} />;
}

export function KeptEnvironmentRow({ entry, hostOnly = false, onJob }: { entry: RemovalEntry; hostOnly?: boolean; onJob?: (id: string, environmentId: string) => void }) {
  const { t } = useTranslation();
  const removal = entry.removal ?? null;
  if (removal === null) return <EmptyState compact title={t('trace.noContent')} icon={<Eye aria-hidden />} />;
  return <ReadyKeptEnvironmentRow entry={{ ...entry, removal }} hostOnly={hostOnly} onJob={onJob} />;
}
function ReadyKeptEnvironmentRow({ entry, hostOnly = false, onJob }: { entry: DeletedEnvironment; hostOnly?: boolean; onJob?: (id: string, environmentId: string) => void }) {
  const { t } = useTranslation(); const cache = useQueryClient(); const [purging, setPurging] = useState<DeletedEnvironment | null>(null); const [jobId, setJobId] = useState<string | null>(entry.removal.job?.id ?? null);
  const refresh = () => { void cache.invalidateQueries({ queryKey: ['trace-deleted'] }); void cache.invalidateQueries({ queryKey: ['deleted-inspection'] }); void cache.invalidateQueries({ queryKey: ['host'] }); void cache.invalidateQueries({ queryKey: ['hosts'] }); };
  const restore = useMutation({ mutationFn: () => restoreEnvironment(entry.environment.id, entry.removal.revision, newActKey()), onSuccess: (job) => { if (onJob === undefined) setJobId(job.id); else onJob(job.id, entry.environment.id); refresh(); }, onError: (error) => { if (error instanceof ApiError) refresh(); } });
  const facts = entry.removal;
  useEffect(() => { if (facts.job !== null && onJob !== undefined) onJob(facts.job.id, entry.environment.id); }, [facts.job?.id, onJob, entry.environment.id]); // eslint-disable-line react-hooks/exhaustive-deps
  return <div className="flex min-w-0 flex-col gap-3" data-kept-environment={entry.environment.id}><RemovalFacts entry={entry} />{!hostOnly && <p className="text-sm text-text-muted">{t('trace.restoreStopped')}</p>}<div className="flex flex-wrap gap-2">{!hostOnly && facts.can_inspect && <Button size="sm" action="show" asChild><a href={inspectionHref('environment', entry.environment.id)}>{t('trace.inspect')}</a></Button>}{!hostOnly && facts.can_restore && <Button size="sm" icon={<RotateCcw aria-hidden />} pending={restore.isPending} onClick={() => restore.mutate()}>{t('trace.restore')}</Button>}{!hostOnly && facts.can_purge && <Button size="sm" variant="danger-ghost" icon={<Trash2 aria-hidden />} onClick={() => setPurging(entry)}>{t('trace.purge')}</Button>}</div>{restore.error !== null && <ErrorAlert error={restore.error} />}{hostOnly && facts.job !== null && <p className="text-sm wrap-anywhere text-text-muted">{retainedJobWords(t,facts.job)}</p>}{!hostOnly && jobId !== null && onJob === undefined && <JobProgress id={jobId} environmentId={entry.environment.id} />}{purging !== null && <PurgeDialog entry={purging} onClose={() => setPurging(null)} onAccepted={(id) => { if (onJob === undefined) setJobId(id); else onJob(id, entry.environment.id); refresh(); }} onRefused={refresh} />}</div>;
}
export function KeptEnvironments({ node, open, hideEmpty = false }: { node: string; open: boolean; hideEmpty?: boolean }) {
  const { t } = useTranslation();
  const [jobs, setJobs] = useState<{ id: string; environmentId: string }[]>([]);
  const onJob = useCallback((id: string, environmentId: string) => setJobs((held) => held.some((job) => job.id === id) ? held : [...held, { id, environmentId }]), []);
  const list = useInfiniteQuery({ ...keptEnvironmentsQuery(node), enabled: open }); const rows = list.isError ? [] : list.data?.pages.flatMap((page) => page.items) ?? [];
  if (hideEmpty && list.isSuccess && rows.length === 0 && jobs.length === 0) return null;
  return <section aria-label={t('trace.keptTitle')} className="mt-6 flex flex-col gap-3"><h3 className="font-medium">{t('trace.keptTitle')}</h3>{jobs.map((job) => <JobProgress key={job.id} id={job.id} environmentId={job.environmentId} />)}<p className="text-sm text-text-muted">{t('trace.keptLead')}</p>{list.isPending ? <SkeletonRows rows={3} /> : list.isError ? <ErrorState compact title={t('trace.environmentReadFailure')} error={list.error} onRetry={() => void list.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('trace.keptEmpty')} icon={<RotateCcw aria-hidden />} /> : <ul className="divide-y divide-border">{rows.map((row) => <li key={row.environment.id} className="py-4"><KeptEnvironmentRow entry={row} onJob={onJob} /></li>)}</ul>}<Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} /></section>;
}
/** Metadata only: no Files, shells, starting control or environment feed mounts here. */
export function RetainedEnvironmentInspection({ id }: { id: string }) {
  const { t } = useTranslation(); const read = useQuery(retainedEnvironmentQuery(id));
  useResourceHeading('environments', Boxes, read.data?.name ?? t('trace.inspect'));
  if (read.isPending) return <SkeletonRows rows={3} />;
  if (read.isError) return <ErrorState error={read.error} title={t('trace.inspectFailure')} onRetry={() => void read.refetch()} />;
  const env = read.data; const removal = env.removal ?? null; const inspection = env.inspection ?? null;
  if (removal === null || inspection === null) return <EmptyState title={t('trace.noContent')} icon={<Eye aria-hidden />} />;
  const entry: DeletedEnvironment = { environment: { id: env.id, name: env.name, address: env.address, owner: env.owner, placement: env.placement, status: env.status, ports_exposed: env.ports_exposed }, home: env.home, host: env.host, storage: env.workspace_storage, removal };
  return <><InspectionBanner facts={inspection} /><KeptEnvironmentRow entry={entry} hostOnly /></>;
}
