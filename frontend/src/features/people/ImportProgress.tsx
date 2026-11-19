/** A running import remains cancellable and leads to its retained report when it ends. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { cancelJob, exportKeys, jobQuery } from '@/api/exports';
import { importForJob, importKeys } from '@/api/imports';
import { Hint } from '@/features/help/Hint';
import { importIdOf } from '@/features/people/importJobs';
import { importReason } from '@/features/people/importReason';
import type { Job } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatSpan, useRunningClock } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Meter } from '@/ui/Meter';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';

const PHASES = new Set(['validate', 'map', 'create', 'link', 'report', 'remove', 'finish']);

function Activity({ job }: { job: Job }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const active = job.state === 'queued' || job.state === 'running';
  const now = useRunningClock(active);
  const updated = job.progress.activity?.updated_at ?? job.started_at ?? job.created_at;
  const threshold = job.progress.activity?.stall_after_seconds;
  const stalled = active && threshold !== undefined && threshold > 0 && now - Date.parse(updated) >= threshold * 1000;
  const elapsed = (active ? now : Date.parse(job.finished_at ?? updated)) - Date.parse(job.started_at ?? job.created_at);
  const phase = job.state === 'queued' ? t('importWizard.run.queued') : PHASES.has(job.progress.phase)
    ? t(`importWizard.run.${job.progress.phase}` as 'importWizard.run.validate') : t('importWizard.run.working');
  return <p role="status" className="text-sm text-text-muted tabular-nums" aria-live="polite">
    {t('importWizard.run.line', {
      phase, done: formatNumber(job.progress.files_done, format.locale), total: job.progress.files_total === null ? '–' : formatNumber(job.progress.files_total, format.locale),
      bytes: formatBytes(job.progress.bytes_done, format.locale), elapsed: Number.isFinite(elapsed) ? formatSpan(elapsed, format, 'clock') : '–',
    })}
    {stalled && <span className="text-warning-text"> · {t('importWizard.run.stalled')}</span>}
  </p>;
}

export function ImportProgress({ id, onReport }: { id: string; onReport: (id: string) => void }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const cache = useQueryClient();
  const job = useQuery(jobQuery(id));
  const done = job.data !== undefined && job.data.state !== 'queued' && job.data.state !== 'running';
  const supplied = job.data === undefined ? null : importIdOf(job.data);
  const lookup = useQuery({ queryKey: importKeys.forJob(id), queryFn: ({ signal }) => importForJob(id, signal), enabled: done && supplied === null, retry: false });
  const report = supplied ?? lookup.data?.id ?? null;
  const stop = useMutation({ mutationFn: () => cancelJob(id), onSuccess: (value) => cache.setQueryData(exportKeys.job(id), value) });
  useEffect(() => { if (done && report !== null) onReport(report); }, [done, onReport, report]);
  if (job.isPending) return <SkeletonRows rows={3} />;
  if (job.isError) return <ErrorState compact error={job.error} onRetry={() => void job.refetch()} />;
  const current = job.data;
  return <div className="flex flex-col gap-3">
    <div className="flex items-center gap-2"><span className="text-sm font-medium">{t('importWizard.steps.run')}</span><Hint id="import-run" /></div>
    <Activity job={current} />
    <Meter look="progress" value={current.progress.percent} label={t('importWizard.steps.run')} />
    {(current.state === 'queued' || current.state === 'running') && <Button size="sm" className="self-start" pending={stop.isPending} onClick={() => stop.mutate()}>{t('importWizard.run.cancel')}</Button>}
    {stop.isError && <ErrorAlert error={stop.error} />}
    {(current.state === 'failed' || current.state === 'lost') && <InlineAlert tone="danger">{current.error === null ? t('importWizard.run.failed') : importReason(current.error, t, format.locale)}</InlineAlert>}
    {current.state === 'cancelled' && <StateLine compact message={t('importWizard.run.cancelled')} />}
    {done && report === null && (lookup.isPending ? <SkeletonRows rows={1} /> : lookup.isError ? <ErrorAlert error={lookup.error} action={<Button size="sm" onClick={() => void lookup.refetch()}>{t('common.tryAgain')}</Button>} /> : <StateLine compact message={t('importWizard.run.reportMissing')} action={<Button asChild size="sm"><Link to="/settings/imports">{t('importWizard.mine.title')}</Link></Button>} />)}
  </div>;
}
