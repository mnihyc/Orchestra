/** Background counts share the existing plan rows and ordinary live job feeds. */
import { useMutation, useQueries, useQueryClient } from '@tanstack/react-query';
import { X } from 'lucide-react';
import { createContext, useContext, useEffect, useId, useRef, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { record } from '@/api/cache';
import { cancelJob, forkKeys, jobQuery } from '@/api/forks';
import type { FilesPlan, Job, Measurement, PlanMount } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatTimeOfDay, useRunningClock } from '@/lib/time';
import { Button } from '@/ui/Button';
import { IconButton } from '@/ui/IconButton';
import { StateLine } from '@/ui/StateLine';
import { ErrorAlert } from '@/ui/ErrorState';

export interface PlanCounts { files?: FilesPlan | null; mounts?: readonly { copy?: PlanMount['copy']; fingerprint_measurement?: PlanMount['fingerprint_measurement'] }[]; measurements?: readonly Measurement[]; plans?: readonly PlanCounts[]; homeFallback?: boolean }
interface Counted { measurement: Measurement; home: boolean; fallback: boolean; fingerprint?: boolean }
interface View { job: Job | undefined; failed: boolean; error: unknown }
interface Counts {
  active: boolean;
  views: ReadonlyMap<string, View>;
  total: number;
  pending: number;
  failed: boolean;
  stopped: boolean;
  retry: () => void;
}
const Context = createContext<Counts | null>(null);

/** The same live read views serve content checks and ordinary size slots. */
export function usePlanMeasurements() { return useContext(Context); }

/** The physical data counted by this plan, once each even when choices share a count. */
export function countsOf(plan: PlanCounts | undefined): Counted[] {
  const values: Counted[] = [];
  const add = (measurement: Measurement | null | undefined, home = false) => { if (measurement !== undefined && measurement !== null) values.push({ measurement, home, fallback: home && plan?.homeFallback === true }); };
  add(plan?.files?.home?.measurement, true);
  add(plan?.files?.file_states?.measurement);
  for (const measurement of plan?.measurements ?? []) add(measurement);
  for (const view of plan?.files?.private_views ?? []) add(view.measurement);
  for (const mount of plan?.mounts ?? []) {
    add(mount.copy?.measurement); add(mount.copy?.holds?.measurement);
    if (mount.fingerprint_measurement !== undefined && mount.fingerprint_measurement !== null) values.push({ measurement: mount.fingerprint_measurement, fingerprint: true, home: false, fallback: false });
  }
  for (const nested of plan?.plans ?? []) values.push(...countsOf(nested));
  return [...new Map(values.map((value) => [value.measurement.job_id, value])).values()];
}

/** Whether the plan needs a completed count before it can be confirmed. */
export function waitsForCount(plan: { blockers: readonly { code: string }[] } | undefined): boolean {
  return plan?.blockers.some((blocker) => blocker.code === 'measurement_pending') ?? false;
}

/** A headless owner of the plan's count subscriptions and its single automatic replan. */
export function Measurements({ plan, active, onReady, children, homeFallback = false }: {
  plan: PlanCounts | undefined; active: boolean; onReady: () => void; children: ReactNode; homeFallback?: boolean;
}) {
  const counts = countsOf(plan);
  const pending = counts.filter((count) => count.measurement.measured_at === null);
  const queries = useQueries({ queries: pending.map((count) => ({ ...jobQuery(count.measurement.job_id), enabled: active })) });
  const refreshed = useRef(new Set<string>());
  // Independent read groups settle independently: a stopped check cannot hold a completed integrity count.
  const readyKeys = [false, true].flatMap((fingerprint) => {
    const group = pending.map((count, index) => ({ count, query: queries[index] })).filter(({ count }) => (count.fingerprint === true) === fingerprint);
    return group.length > 0 && group.every(({ count, query }) => query?.data?.state === 'succeeded' || count.fingerprint !== true && (homeFallback && count.home || count.fallback) && query?.data?.state === 'failed' && query.data.error?.code === 'unsupported')
      ? [group.map(({ count }) => count.measurement.job_id).sort().join(',')] : [];
  });
  const key = readyKeys.join(';');
  useEffect(() => {
    if (active && key !== '' && !refreshed.current.has(key)) { refreshed.current.add(key); onReady(); }
  }, [active, key, onReady]);
  const sizes = queries.filter((_, index) => pending[index]?.fingerprint !== true);
  const value: Counts = {
    active, total: counts.filter((count) => count.fingerprint !== true).length,
    pending: sizes.filter((query) => !query.isError && (query.data === undefined || query.data.state === 'queued' || query.data.state === 'running')).length,
    failed: sizes.some((query) => query.isError || query.data?.state === 'failed' || query.data?.state === 'lost'),
    stopped: sizes.some((query) => query.data?.state === 'cancelled'),
    views: new Map(pending.map((count, index) => [count.measurement.job_id, { job: queries[index]?.data, failed: queries[index]?.isError ?? false, error: queries[index]?.error ?? null }])),
    retry: onReady,
  };
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

/** The count-dependent reason occupies the dialog's existing reason line beside confirmation. */
export function MeasurementReason({ waiting, planning, fallback }: { waiting: boolean; planning: boolean; fallback: string | null }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const counts = useContext(Context);
  if (!waiting) return fallback;
  if (planning) return t('liveWork.planning');
  if (counts !== null && counts.pending > 0) return t('liveWork.countLocations', { count: counts.total, pending: formatNumber(counts.pending, locale), total: formatNumber(counts.total, locale) });
  if (counts?.failed === true) return t('liveWork.countFailed');
  if (counts?.stopped === true) return t('liveWork.countStopped');
  return t('liveWork.planning');
}

/** The size slot of an existing plan row, with its retained count identity. */
export interface MeasurementSizeData { reference: Measurement; measured: boolean; file_count: number | null; total_bytes: number }

function complete(job: Job | undefined): { files: number; bytes: number } | null {
  if (job?.state !== 'succeeded') return null;
  const result = record(job.result) ? job.result : {};
  const first: unknown = Array.isArray(result.volumes) ? result.volumes[0] : result;
  const measured = record(first) && record(first.measure) ? first.measure : record(first) ? first : {};
  if (typeof measured.files === 'number' && typeof measured.bytes === 'number') return { files: measured.files, bytes: measured.bytes };
  return typeof measured.file_count === 'number' && typeof measured.total_bytes === 'number'
    ? { files: measured.file_count, bytes: measured.total_bytes } : null;
}

/** Live counting, stop and retry stay in the row where its measured size belongs. */
export function MeasurementSize({ size }: { size: MeasurementSizeData }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const cache = useQueryClient();
  const counts = useContext(Context);
  const id = useId();
  const view = counts?.views.get(size.reference.job_id);
  const job = view?.job;
  const cancel = useMutation({ mutationFn: (jobId: string) => cancelJob(jobId), onSuccess: (stopped) => cache.setQueryData(forkKeys.job(stopped.id), stopped) });
  const active = !size.measured && size.reference.measured_at === null && (counts?.active ?? true) && view?.failed !== true && (job === undefined || job.state === 'queued' || job.state === 'running');
  const now = useRunningClock(active);
  const activity = job?.progress.activity;
  const stalled = active && activity !== undefined && activity !== null && activity.stall_after_seconds > 0 && now - Date.parse(activity.updated_at) >= activity.stall_after_seconds * 1000;
  const currentCancel = cancel.variables === size.reference.job_id;
  const stopping = currentCancel && cancel.isSuccess || activity?.stopping === true;
  const finished = complete(job);
  const files = size.measured ? size.file_count : finished?.files;
  const bytes = size.measured ? size.total_bytes : finished?.bytes;
  const total = (fileCount: number | null | undefined, byteCount: number) => [
    fileCount === null || fileCount === undefined ? null : t('liveWork.files', { count: fileCount, value: formatNumber(fileCount, format.locale) }),
    formatBytes(byteCount, format.locale),
  ].filter((part) => part !== null).join(' · ');
  const directories = activity?.directories_done;
  const title = directories === null || directories === undefined ? undefined : t('liveWork.directories', { count: directories, value: formatNumber(directories, format.locale) });
  if (bytes !== undefined) return <span className="text-sm text-text-muted tabular-nums" data-measured-size>{total(files, bytes)}</span>;
  if (job?.state === 'succeeded' || size.reference.measured_at !== null) return <span className="text-sm text-text-muted">{t('liveWork.notCounted')}</span>;
  const retry = counts === null ? undefined : <Button size="sm" variant="ghost" onClick={counts.retry}>{t('liveWork.retry')}</Button>;
  if (job?.state === 'cancelled') return <StateLine compact role="status" className="inline-flex max-w-full text-text-muted [&_[data-state-copy]]:basis-auto [&_[data-state-copy]]:flex-none" message={t('liveWork.countStopped')} action={retry} />;
  if (view?.failed === true || job?.state === 'failed') return <ErrorAlert className="inline-flex max-w-full [&_[data-state-copy]]:basis-auto [&_[data-state-copy]]:flex-none" error={view?.error ?? job?.error ?? new Error(t('liveWork.countFailed'))} message={t('liveWork.countFailed')} action={retry} />;
  return <span className="inline-flex min-w-0 flex-wrap items-center gap-1 text-sm text-text-muted" data-counting-size title={title}>
    <span id={id} aria-live="polite" className="tabular-nums">{t(stopping ? 'liveWork.stopping' : 'liveWork.counting')}{!stopping && job !== undefined ? ' ' + total(job.progress.files_done, job.progress.bytes_done) : ''}</span>
    {stalled && !stopping && <span className="text-warning-text">{' · '}{t('liveWork.unchanged', { time: formatTimeOfDay(activity.updated_at, format, now) })}</span>}
    <IconButton size="sm" action="cancel" icon={<X aria-hidden />} label={t(stopping ? 'liveWork.stopping' : 'liveWork.cancelCount')} aria-describedby={id} pending={currentCancel && cancel.isPending} disabled={stopping} reason={currentCancel && cancel.isError ? t('liveWork.stopFailed') : undefined} onClick={() => cancel.mutate(size.reference.job_id)} />
  </span>;
}
