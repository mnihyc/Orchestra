/** Import jobs remain observable after their dialog closes, and notify only when requested. */
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { create, useStore } from 'zustand';

import { client } from '@/api/client';
import { exportKeys, jobQuery } from '@/api/exports';
import { importForJob, importKeys } from '@/api/imports';
import { queryClient } from '@/api/query';
import type { Job } from '@/generated/api-types';
import { toast } from '@/ui/Toast';

const REMEMBER = 'orch.imports.watched';

export interface WatchedImport {
  job: string;
  place: string | null;
  report: string | null;
  undo: boolean;
  notify: boolean;
  state: 'running' | 'done' | 'failed' | 'cancelled';
  percent: number;
  reconnecting: boolean;
}

interface WatchStore { user: string | null; items: readonly WatchedImport[] }
export const useWatchedImports = create<WatchStore>(() => ({ user: null, items: [] }));

function save(): void {
  const { user, items } = useWatchedImports.getState();
  try {
    if (user === null || items.length === 0) sessionStorage.removeItem(REMEMBER);
    else sessionStorage.setItem(REMEMBER, JSON.stringify({ user, items }));
  } catch { /* The job is still available from the import history. */ }
}

function change(job: string, update: (item: WatchedImport) => WatchedImport | null): void {
  useWatchedImports.setState((state) => ({ items: state.items.flatMap((item) => {
    if (item.job !== job) return [item];
    const next = update(item);
    return next === null ? [] : [next];
  }) }));
  save();
}

/** The result's import id, when the job supplied one. */
export function importIdOf(job: Job): string | null {
  const result = job.result;
  return typeof result === 'object' && result !== null && 'import_id' in result && typeof result.import_id === 'string' ? result.import_id : null;
}

/** Follows a job after the server accepts it. */
export function watchImport(job: Job, place: string | null, notify: boolean, undo = false, report: string | null = null): void {
  const user = client.session.getState().userId;
  if (user === null) return;
  const held = useWatchedImports.getState();
  if (held.user === user && held.items.some((item) => item.job === job.id)) return;
  queryClient.setQueryData(exportKeys.job(job.id), job);
  useWatchedImports.setState((state) => {
    const existing = state.user === user ? state.items.find((item) => item.job === job.id) : undefined;
    if (existing !== undefined) return state;
    return { user, items: [
      ...(state.user === user ? state.items : []),
      { job: job.id, place, report: report ?? importIdOf(job), undo, notify, state: 'running', percent: job.progress.percent, reconnecting: false },
    ] };
  });
  save();
}

/** Removes a finished status after it has been opened. */
export function dismissImport(job: string): void { change(job, () => null); }

function restore(user: string): void {
  let items: WatchedImport[] = [];
  try {
    const saved: unknown = JSON.parse(sessionStorage.getItem(REMEMBER) ?? 'null');
    if (typeof saved === 'object' && saved !== null && 'user' in saved && saved.user === user && 'items' in saved && Array.isArray(saved.items)) {
      const candidates: unknown[] = saved.items;
      items = candidates.filter((item): item is WatchedImport => {
        if (typeof item !== 'object' || item === null) return false;
        const value = item as Record<string, unknown>;
        return typeof value.job === 'string' && (typeof value.place === 'string' || value.place === null)
          && (typeof value.report === 'string' || value.report === null) && typeof value.undo === 'boolean' && typeof value.notify === 'boolean'
          && (value.state === 'running' || value.state === 'done' || value.state === 'failed' || value.state === 'cancelled')
          && typeof value.percent === 'number' && typeof value.reconnecting === 'boolean';
      });
    }
  } catch { /* An unreadable saved state is not an import record. */ }
  useWatchedImports.setState({ user, items });
}

/** Subscribes to watched jobs, including ones restored after a reload. */
export function useImportJobWatch(onOpen: (item: WatchedImport) => void): void {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const user = useStore(client.session, (state) => state.userId);
  const reader = useStore(client.session, (state) => state.readerVersion ?? 0);
  const watched = useWatchedImports((state) => state);
  const items = user !== null && watched.user === user ? watched.items : [];
  const running = items.filter((item) => item.state === 'running');
  const jobs = useQueries({ queries: running.map((item) => jobQuery(item.job)) });
  const needsReport = items.filter((item) => item.state !== 'running' && item.report === null);
  const reports = useQueries({ queries: needsReport.map((item) => ({
    queryKey: importKeys.forJob(item.job), queryFn: ({ signal }: { signal: AbortSignal }) => importForJob(item.job, signal), retry: false,
  })) });

  useEffect(() => {
    if (user === null) { useWatchedImports.setState({ user: null, items: [] }); save(); return; }
    if (useWatchedImports.getState().user !== user) restore(user);
  }, [user]);

  useEffect(() => {
    if (user === null || client.session.getState().userId !== user || (client.session.getState().readerVersion ?? 0) !== reader || useWatchedImports.getState().user !== user) return;
    running.forEach((held, index) => {
      const query = jobs[index];
      const job = query?.data;
      if (job === undefined) {
        if (query?.isError && !held.reconnecting) change(held.job, (one) => ({ ...one, reconnecting: true }));
        return;
      }
      if (job.state === 'queued' || job.state === 'running') {
        if (held.percent !== job.progress.percent || held.reconnecting) change(held.job, (one) => ({ ...one, percent: job.progress.percent, reconnecting: false }));
        return;
      }
      const state = job.state === 'succeeded' ? 'done' : job.state === 'cancelled' ? 'cancelled' : 'failed';
      const report = importIdOf(job) ?? held.report;
      change(held.job, (one) => ({ ...one, state, percent: job.progress.percent, report, reconnecting: false }));
      void cache.invalidateQueries({ queryKey: importKeys.mine(), exact: true });
      if (report !== null) void cache.invalidateQueries({ queryKey: importKeys.one(report), exact: true });
      if (held.notify) {
        const title = held.undo && state === 'done' ? t('importWizard.job.undone') : t(state === 'done' ? 'importWizard.job.finished' : state === 'cancelled' ? 'importWizard.job.cancelled' : 'importWizard.job.failed');
        toast({ tone: state === 'failed' ? 'danger' : 'info', title, action: { label: t('importWizard.job.viewReport'), run: () => {
          const current = useWatchedImports.getState().items.find((item) => item.job === held.job);
          if (current !== undefined) onOpen(current);
        } } });
      }
    });
  }, [cache, jobs, onOpen, reader, running, t, user]);

  useEffect(() => {
    if (user === null || client.session.getState().userId !== user || (client.session.getState().readerVersion ?? 0) !== reader || useWatchedImports.getState().user !== user) return;
    needsReport.forEach((item, index) => {
      const found = reports[index]?.data;
      if (found !== null && found !== undefined) change(item.job, (one) => ({ ...one, report: found.id }));
    });
  }, [needsReport, reader, reports, user]);
}

