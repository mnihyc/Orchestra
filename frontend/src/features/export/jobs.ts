/** The archives being written: which jobs this page follows, how far each has come, and the notice when one ends, with Download.
 *
 * Asking for an archive starts a job. The page follows its live rows while it runs, and keeps
 * the ones it follows for a reload of the tab. When a job ends the person is told
 * where they are: a ready archive offers Download, a failed one says why, a cancelled one is simply gone.
 * The status bar shows what runs and what has just ended.
 */
import type { TFunction } from 'i18next';
import { useQueries } from '@tanstack/react-query';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { create } from 'zustand';

import { jobQuery } from '@/api/exports';
import type { Job, JobError } from '@/generated/api-types';
import { toast } from '@/ui/Toast';
import { errorText } from '@/ui/errorText';
import { currentLocale } from '@/i18n';

import { fetchArchive } from './download';

/** Where the jobs followed are kept, so that a reload of the tab goes on following them. */
const REMEMBER = 'orch.exports.watched';

export type WatchState = 'running' | 'ready' | 'failed';

export interface Watched {
  job: string;
  /** The name of the place, for the words. */
  place: string;
  state: WatchState;
  percent: number;
  phase: string;
  /** The archive, once the job has made it. */
  exportId: string | null;
  error: JobError | null;
}

interface WatchStore {
  items: readonly Watched[];
}

export const useWatched = create<WatchStore>(() => ({ items: [] }));

function change(job: string, make: (one: Watched) => Watched | null): void {
  useWatched.setState((state) => ({
    items: state.items.flatMap((one) => {
      if (one.job !== job) return [one];
      const next = make(one);
      return next === null ? [] : [next];
    }),
  }));
}

function remember(): void {
  try {
    const running = useWatched.getState().items.filter((one) => one.state === 'running').map((one) => ({ job: one.job, place: one.place }));
    if (running.length === 0) sessionStorage.removeItem(REMEMBER);
    else sessionStorage.setItem(REMEMBER, JSON.stringify(running));
  } catch {
    // The page works without it: a reload simply stops following.
  }
}

/** Starts following a job that writes an archive of `place`. */
export function watchExport(job: Job, place: string): void {
  useWatched.setState((state) => ({
    items: [
      ...state.items.filter((one) => one.job !== job.id),
      { job: job.id, place, state: 'running', percent: job.progress.percent, phase: job.progress.phase, exportId: null, error: null },
    ],
  }));
  remember();
}

/** Stops showing a job that has ended. */
export function dismissWatched(job: string): void {
  change(job, () => null);
}

/** Stops showing every job that has ended: the person has seen them. */
export function dismissFinished(): void {
  useWatched.setState((state) => ({ items: state.items.filter((one) => one.state === 'running') }));
}

/** The words for why a job failed: the refusal of the code where there is one, else what the server said. */
export function failureWords(error: JobError | null, t: TFunction): string {
  if (error === null) return t('export.job.failedAbout');
  if (error.code === 'export.too_large') return t('errors.export.too_large');
  if (error.code === 'export.too_many') return t('errors.export.too_many');
  if (error.code === 'export.nothing') return t('errors.export.nothing');
  return errorText(error, t, { locale: currentLocale() }).message;
}

/** What an archive's job says it made: the identifier of the archive. */
export function exportOf(job: Job): string | null {
  const made = (job.result as { export_id?: unknown } | null)?.export_id;
  return typeof made === 'string' ? made : null;
}

/** Takes what the server says of a job: how far it is, or how it ended, and tells the person if it ended. */
function settle(job: Job, t: TFunction): void {
  const held = useWatched.getState().items.find((one) => one.job === job.id);
  if (held?.state !== 'running') return;
  if (job.state === 'queued' || job.state === 'running') {
    if (held.percent === job.progress.percent && held.phase === job.progress.phase) return;
    change(job.id, (one) => ({ ...one, percent: job.progress.percent, phase: job.progress.phase }));
    return;
  }
  if (job.state === 'cancelled') {
    change(job.id, () => null);
    remember();
    return;
  }
  if (job.state === 'failed') {
    change(job.id, (one) => ({ ...one, state: 'failed', percent: job.progress.percent, error: job.error }));
    toast({ tone: 'danger', title: t('export.job.failed', { place: held.place }), description: failureWords(job.error, t) });
  } else {
    const exportId = exportOf(job);
    change(job.id, (one) => ({ ...one, state: 'ready', percent: 100, exportId }));
    toast({
      tone: 'success',
      title: t('export.job.ready', { place: held.place }),
      ...(exportId === null ? {} : { action: { label: t('export.downloadButton'), run: () => void fetchArchive(exportId) } }),
    });
  }
  remember();
}

/** Follows the jobs that write archives for as long as one runs, and takes up the ones a reload of the tab left. */
export function useExportWatch(): void {
  const { t } = useTranslation();
  const items = useWatched((state) => state.items);
  const jobs = useQueries({ queries: items.filter((one) => one.state === 'running').map((one) => jobQuery(one.job)) });

  useEffect(() => {
    try {
      const kept = JSON.parse(sessionStorage.getItem(REMEMBER) ?? '[]') as { job?: unknown; place?: unknown }[];
      for (const one of kept) {
        if (typeof one.job !== 'string' || typeof one.place !== 'string') continue;
        const job = one.job;
        const place = one.place;
        if (useWatched.getState().items.some((held) => held.job === job)) continue;
        useWatched.setState((state) => ({ items: [...state.items, { job, place, state: 'running', percent: 0, phase: '', exportId: null, error: null }] }));
      }
    } catch {
      // Nothing kept, or nothing that can be read: nothing to follow.
    }
  }, []);

  useEffect(() => {
    for (const job of jobs) if (job.data !== undefined) settle(job.data, t);
  }, [jobs, t]);
}
