/** How the reader likes a conversation: how much of the agent's work is open, how a difference is laid out, whether the thread's history is shown.
 *
 * The depth of the agent's work and the layout of a difference follow the person to every device
 * (`timeline` in what the interface keeps for them); hiding the lines of a thread's history is a
 * choice of the device they are on. Each is shown as chosen at once and put right if the server refuses.
 */
import { CancelledError, useMutation, useQuery, useQueryClient, type QueryClient } from '@tanstack/react-query';
import { useCallback, useSyncExternalStore } from 'react';

import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import { client } from '@/api/client';
import type { WorkDetail } from '@/features/timeline/rows';
import type { DiffLayout } from '@/ui/DiffView';

/** How much is open when the reader has not chosen. */
export const DEFAULT_WORK_DETAIL: WorkDetail = 'steps';
const DETAILS: readonly WorkDetail[] = ['steps', 'summary', 'everything'];
const HIDE_HISTORY_KEY = 'orch.timeline.hideHistory';

interface Stored {
  work_detail?: WorkDetail;
  diff_layout?: DiffLayout;
  live_tools?: boolean;
}

export type ThreadViewMode = 'focused' | 'full' | 'live';

const record = (value: unknown): Record<string, unknown> =>
  typeof value === 'object' && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};

function readStored(state: Record<string, unknown> | undefined): Stored {
  const value = state?.timeline;
  if (typeof value !== 'object' || value === null) return {};
  const stored = value as Record<string, unknown>;
  return {
    ...(DETAILS.includes(stored.work_detail as WorkDetail) ? { work_detail: stored.work_detail as WorkDetail } : {}),
    ...(stored.diff_layout === 'one' || stored.diff_layout === 'two' ? { diff_layout: stored.diff_layout } : {}),
    ...(typeof stored.live_tools === 'boolean' ? { live_tools: stored.live_tools } : {}),
  };
}

interface Change {
  user: string | null;
  reader: number;
  thread?: string;
  value: Stored;
}

/** Keep unrecognized preferences and other threads when one local choice changes. */
export function changedTimeline(value: unknown, change: Pick<Change, 'thread' | 'value'>): Record<string, unknown> {
  const held = record(value);
  if (change.thread === undefined) return { ...held, ...change.value };
  const threads = record(held.threads);
  return { ...held, threads: { ...threads, [change.thread]: { ...record(threads[change.thread]), ...change.value } } };
}

/** Roll back only this attempted value; newer choices and unrelated personal fields stay intact. */
export function rollbackTimeline(value: unknown, beforeValue: unknown, change: Pick<Change, 'thread' | 'value'>): Record<string, unknown> {
  const held = record(value), before = record(beforeValue);
  const restore = (value: Record<string, unknown>, previous: Record<string, unknown>) => {
    const next = { ...value };
    let restored = false;
    for (const key of ['work_detail', 'diff_layout', 'live_tools'] as const) {
      if (change.value[key] === undefined || value[key] !== change.value[key]) continue;
      if (Object.hasOwn(previous, key)) next[key] = previous[key];
      else Reflect.deleteProperty(next, key);
      restored = true;
    }
    return { next, restored };
  };
  if (change.thread === undefined) return restore(held, before).next;
  const threads = record(held.threads), previous = record(before.threads);
  const target = restore(record(threads[change.thread]), record(previous[change.thread]));
  if (!target.restored) return held;
  const next = { ...threads };
  if (!Object.hasOwn(previous, change.thread) && Object.keys(target.next).length === 0) Reflect.deleteProperty(next, change.thread);
  else next[change.thread] = target.next;
  const timeline = { ...held };
  if (!Object.hasOwn(before, 'threads') && Object.keys(next).length === 0) delete timeline.threads;
  else timeline.threads = next;
  return timeline;
}

const writes = new Map<string, Promise<void>>();
const current = (change: Change) => change.user !== null && client.session.getState().userId === change.user
  && (client.session.getState().readerVersion ?? 0) === change.reader;

/** All mounted controls share one writer for the personal namespace and current reader. */
function writeTimeline(cache: QueryClient, change: Change): Promise<void> {
  const key = JSON.stringify([change.user, change.reader]);
  const before = writes.get(key) ?? Promise.resolve();
  const pending = before.catch(() => undefined).then(async () => {
    if (!current(change)) throw new CancelledError({ silent: true });
    const state = cache.getQueryData<Record<string, unknown>>(meStateKey);
    await setMeState('timeline', changedTimeline(state?.timeline, change));
  });
  writes.set(key, pending);
  return pending.finally(() => { if (writes.get(key) === pending) writes.delete(key); });
}

// The device's own choice: kept in the browser, and followed by every conversation that is open.
const listeners = new Set<() => void>();
function readHidden(): boolean {
  try {
    return localStorage.getItem(HIDE_HISTORY_KEY) === '1';
  } catch {
    return false;
  }
}
let hidden = readHidden();

function setHidden(next: boolean): void {
  hidden = next;
  try {
    localStorage.setItem(HIDE_HISTORY_KEY, next ? '1' : '0');
  } catch {
    // The choice holds for this page even where the browser keeps nothing.
  }
  for (const listener of listeners) listener();
}

export interface TimelinePreferences {
  detail: WorkDetail;
  setDetail: (detail: WorkDetail) => void;
  /** Live changes expansion/following policy while the existing depth still owns folding. */
  liveTools: boolean;
  setViewMode: (mode: ThreadViewMode) => void;
  layout: DiffLayout;
  setLayout: (layout: DiffLayout) => void;
  hideHistory: boolean;
  setHideHistory: (hide: boolean) => void;
}

export function useTimelinePreferences(threadId?: string): TimelinePreferences {
  const queryClient = useQueryClient();
  const state = useQuery(meStateQuery);
  const global = readStored(state.data);
  const timeline = record(state.data?.timeline);
  const own: Stored = threadId === undefined ? {} : readStored({ timeline: record(record(timeline.threads)[threadId]) });
  const detail = own.work_detail ?? global.work_detail ?? DEFAULT_WORK_DETAIL;
  const save = useMutation({
    mutationFn: (change: Change) => writeTimeline(queryClient, change),
    onMutate: async (change) => {
      if (!current(change)) throw new CancelledError({ silent: true });
      // An unopened bag must arrive before a full namespace write can preserve its other fields.
      const before = queryClient.getQueryData<Record<string, unknown>>(meStateKey) ?? await queryClient.query(meStateQuery);
      if (!current(change)) throw new CancelledError({ silent: true });
      const next = changedTimeline(before.timeline, change);
      queryClient.setQueryData<Record<string, unknown>>(meStateKey, { ...before, timeline: next });
      return { before, next };
    },
    onError: (_error, change, context) => {
      if (!current(change)) return;
      const held = queryClient.getQueryData<Record<string, unknown>>(meStateKey);
      if (context !== undefined) queryClient.setQueryData(meStateKey, { ...held, timeline: rollbackTimeline(held?.timeline, context.before.timeline, change) });
      void queryClient.invalidateQueries({ queryKey: meStateKey, exact: true });
    },
  });
  const choose = useCallback((value: Stored, thread?: string) => {
    const { userId: user, readerVersion } = client.session.getState();
    save.mutate({ user, reader: readerVersion ?? 0, ...(thread === undefined ? {} : { thread }), value });
  }, [save]);
  const hideHistory = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => hidden,
  );
  return {
    detail,
    setDetail: useCallback((detail) => choose({ work_detail: detail, live_tools: false }, threadId), [choose, threadId]),
    liveTools: detail !== 'summary' && own.live_tools === true,
    setViewMode: useCallback((mode) => choose({ work_detail: mode === 'focused' ? 'summary' : 'steps', live_tools: mode === 'live' }, threadId), [choose, threadId]),
    layout: global.diff_layout ?? 'one',
    setLayout: useCallback((layout) => choose({ diff_layout: layout }), [choose]),
    hideHistory,
    setHideHistory: setHidden,
  };
}
