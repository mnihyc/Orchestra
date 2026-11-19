/** The bar with its percentage and phase, the checks one by one, and cancellation; the same from any device.
 *
 * What a job that is under way looks like: a bar that never goes back, the phase in words with what is moving (the
 * bytes copied, the time left), the phases as a row of ticks, and, while the new side is being checked, each check as
 * it passes. The job is the server's: Hide closes the view and changes nothing, and the thread, the environment or the
 * provider carries the same bar and opens this view again. Cancel and undo is there until the job commits, asks once and
 * says what undoing means. When the link is lost the bar stays where it was seen, and says that it is looking again.
 */
import { Check, Circle, LoaderCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { Hint } from '@/features/help/Hint';

import type { Job } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { formatExact, formatSpan, formatTimeOfDay, useRunningClock } from '@/lib/time';
import { cx } from '@/ui/cx';
import { InlineAlert } from '@/ui/InlineAlert';
import { Meter } from '@/ui/Meter';
import { CallRecoveryStatus } from '@/features/calls/Status';
import { CallHelp } from '@/features/calls/Help';

/** Whether a job is past the point after which nothing is undone: its commit has happened. */
export function committed(job: Job): boolean {
  return job.progress.phases.find((phase) => phase.name === 'commit')?.state === 'done';
}

/** The checks of the new side, as the job has printed them, one to a line. */
export function checksOf(job: Job): string[] {
  return (job.log ?? '')
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '');
}

/** File phases whose byte counters describe a copy rather than a size count. */
const COPY_PHASES = new Set(['copy', 'files', 'upload', 'download', 'materialize', 'restore', 'apply', 'write', 'volumes', 'hash']);
const SEPARATOR = ' · ';

/** The running phase and its live counters, as compact fragments. */
export function useLine(job: Job): string {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { locale } = format;
  const progress = job.progress;
  if (job.state === 'cancelled' || job.state === 'failed' || job.state === 'succeeded' || job.state === 'lost') return t(`source.job.${job.state}`);
  if (progress.activity?.stopping === true) return t('liveWork.stopping');
  if (job.state === 'queued') return t('change.progress.queued');
  if (progress.activity?.waiting_for?.type === 'environment_change') return t('liveWork.queuedChange');
  if (progress.activity?.waiting_for?.type === 'operations') return t('hostCustody.waitOperations', { number: formatNumber(progress.activity.waiting_for.count, locale), count: progress.activity.waiting_for.count });
  if (progress.activity?.waiting_for?.type === 'host_environments') return t('hostCustody.waitOthers', { number: formatNumber(progress.activity.waiting_for.count, locale), count: progress.activity.waiting_for.count });
  if (progress.activity?.waiting_for?.type === 'retained_data') return t('hostCustody.waitDate', { date: formatExact(progress.activity.waiting_for.until, format) });
  if (progress.activity?.waiting_for?.type === 'machine') return t('liveWork.waitingFor', { machine: progress.activity.waiting_for.name });
  const measuring = progress.phase === 'measure' || job.kind === 'measurement';
  const fingerprinting = progress.phase === 'fingerprint_source' || progress.phase === 'fingerprint_target';
  const copying = !measuring && COPY_PHASES.has(progress.phase);
  const base = fingerprinting ? t(progress.phase === 'fingerprint_source' ? 'change.phase.fingerprint_source.doing' : 'change.phase.fingerprint_target.doing') : measuring ? t('liveWork.countingLabel') : copying ? t('liveWork.copying') : t(`change.phase.${progress.phase}.doing`, { defaultValue: progress.message ?? t('source.job.running') });
  const parts = [base];
  if (job.kind === 'document_notices' && /^\d+$/.test(progress.message ?? '')) {
    const checked = Number(progress.message);
    if (Number.isSafeInteger(checked)) parts.push(t('people.quick.peopleChecked', { number: formatNumber(checked, locale), count: checked }));
  }
  if (measuring || copying || progress.files_done > 0) {
    const done = formatNumber(progress.files_done, locale);
    parts.push(copying && progress.files_total !== null
      ? t('liveWork.filesProgress', { done, total: formatNumber(progress.files_total, locale) })
      : t('liveWork.files', { count: progress.files_done, value: done }));
  }
  if (progress.bytes_total !== null) parts.push(t('change.progress.bytes', { done: formatBytes(progress.bytes_done, locale), total: formatBytes(progress.bytes_total, locale) }));
  else if (measuring || progress.bytes_done > 0) parts.push(formatBytes(progress.bytes_done, locale));
  if (copying && progress.rate_bps !== null && progress.rate_bps > 0) parts.push(t('liveWork.rate', { rate: formatBytes(progress.rate_bps, locale) }));
  if (copying && progress.eta_seconds !== null && progress.eta_seconds > 0) parts.push(t('liveWork.remaining', { time: formatSpan(progress.eta_seconds * 1000, format, 'short') }));
  return parts.join(SEPARATOR);
}

/** One live line for counters, elapsed time and a dated waiting fragment. */
export function JobActivityLine({ job }: { job: Job }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const line = useLine(job);
  const active = job.state === 'queued' || job.state === 'running';
  const now = useRunningClock(active);
  const updated = job.progress.activity?.updated_at ?? job.started_at ?? job.created_at;
  const threshold = job.progress.activity?.stall_after_seconds;
  const waiting = job.progress.activity?.waiting_for;
  const stopping = job.progress.activity?.stopping === true;
  const recovering = job.call_recovery !== null && job.call_recovery !== undefined;
  const stalled = active && !recovering && threshold !== undefined && threshold > 0 && now - Date.parse(updated) >= threshold * 1000;
  const elapsed = (active ? now : Date.parse(job.finished_at ?? updated)) - Date.parse(job.started_at ?? job.created_at);
  const directories = job.progress.activity?.directories_done;
  const title = [directories === null || directories === undefined ? null : t('liveWork.directories', { count: directories, value: formatNumber(directories, format.locale) }), job.progress.message].filter((part) => part !== null && part !== '').join(SEPARATOR);
  const delayedStop = stopping && stalled && waiting?.type === 'machine';
  return <p className="text-sm text-text-muted tabular-nums" aria-live="polite" title={title || undefined} data-job-activity>
    <span data-job-line className={delayedStop ? 'text-warning-text' : undefined}>{delayedStop ? t('liveWork.stoppingOn', { machine: waiting.name }) : job.call_recovery === null || job.call_recovery === undefined ? line : <CallRecoveryStatus recovery={job.call_recovery} service />}</span>
    {Number.isFinite(elapsed) && <span data-job-elapsed className="inline-block min-w-[11ch]">{SEPARATOR}{formatSpan(elapsed, format, 'clock')}</span>}
    {recovering && <CallHelp section="services" />}
    {stalled && !stopping && <span className="text-warning-text" data-job-stalled>{SEPARATOR}{t('liveWork.unchanged', { time: formatTimeOfDay(updated, format, now) })}</span>}
  </p>;
}

function Tick({ state }: { state: Job['progress']['phases'][number]['state'] }) {
  if (state === 'done') return <Check className="text-success" aria-hidden />;
  if (state === 'running') return <LoaderCircle className="animate-spin text-accent" aria-hidden />;
  return <Circle className="text-text-subtle" aria-hidden />;
}

export interface ProgressProps {
  job: Job;
  /** What is being done, as the heading says it: "Switching to Team Pro". */
  heading: string;
  /** The last look failed: the bar is where it was last seen. */
  reconnecting: boolean;
  className?: string;
  /** Resource-specific phases keep the same ticks with their ordinary short words. */
  phaseLabel?: (name: string) => string;
}

export function JobProgress({ job, heading, reconnecting, className, phaseLabel }: ProgressProps) {
  const { t } = useTranslation();
  const checks = checksOf(job);
  const verifying = job.progress.phase === 'verify' && job.state === 'running';
  return (
    <div className={cx('flex flex-col gap-3', className)} data-job-state={job.state} data-job-phase={job.progress.phase}>
      <div className="flex items-baseline justify-between gap-3">
        <div className="flex min-w-0 items-center gap-1"><h3 className="text-base font-semibold text-text">{heading}</h3><Hint id="live-work" popover /></div>
        <span className="text-base font-medium text-text tabular-nums" data-job-percent>
          {job.progress.percent} %
        </span>
      </div>
      <Meter look="progress" value={job.progress.percent} label={heading} />
      <JobActivityLine job={job} />
      <ol aria-label={t('change.progress.phases')} className="flex flex-wrap gap-x-4 gap-y-1 text-sm">
        {job.progress.phases.map((phase) => (
          <li key={phase.name} aria-current={phase.state === 'running' ? 'step' : undefined} data-phase={phase.name} data-phase-state={phase.state} className={cx('flex items-center gap-1', phase.state === 'pending' ? 'text-text-subtle' : phase.state === 'running' ? 'font-medium text-text' : 'text-text-muted')}>
            <Tick state={phase.state} />
            <span>{phaseLabel?.(phase.name) ?? t(`change.phase.${phase.name}.short`, { defaultValue: phase.name })}</span>
            <span className="sr-only">{t(`change.progress.state.${phase.state}`)}</span>
          </li>
        ))}
      </ol>
      {(verifying || checks.length > 0) && job.state === 'running' && (
        <ul aria-label={t('change.progress.checks')} className="flex flex-col gap-1 text-sm text-text-muted" data-job-checks>
          {checks.map((check) => (
            <li key={check} className="flex items-center gap-1">
              <Check className="text-success" aria-hidden /> {t(`change.check.${check.replaceAll(' ', '_')}`, { defaultValue: check })}
            </li>
          ))}
        </ul>
      )}
      {reconnecting && (
        <InlineAlert tone="info" title={t('change.progress.reconnecting')}>
          {t('change.progress.reconnectingAbout')}
        </InlineAlert>
      )}
    </div>
  );
}
