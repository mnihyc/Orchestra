/** How the reader likes a conversation: how much of the agent's work is open, how a difference is laid out, whether the thread's history is shown.
 *
 * The depth of the agent's work and the layout of a difference follow the person to every device
 * (`timeline` in what the interface keeps for them); hiding the lines of a thread's history is a
 * choice of the device they are on. Each is shown as chosen at once and put right if the server refuses.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useSyncExternalStore } from 'react';

import { meStateKey, meStateQuery, setMeState } from '@/api/personal';
import type { WorkDetail } from '@/features/timeline/rows';
import type { DiffLayout } from '@/ui/DiffView';

/** How much is open when the reader has not chosen. */
export const DEFAULT_WORK_DETAIL: WorkDetail = 'steps';
const DETAILS: readonly WorkDetail[] = ['steps', 'summary', 'everything'];
const HIDE_HISTORY_KEY = 'orch.timeline.hideHistory';

interface Stored {
  work_detail?: WorkDetail;
  diff_layout?: DiffLayout;
}

function readStored(state: Record<string, unknown> | undefined): Stored {
  const value = state?.timeline;
  if (typeof value !== 'object' || value === null) return {};
  const stored = value as Record<string, unknown>;
  return {
    ...(DETAILS.includes(stored.work_detail as WorkDetail) ? { work_detail: stored.work_detail as WorkDetail } : {}),
    ...(stored.diff_layout === 'one' || stored.diff_layout === 'two' ? { diff_layout: stored.diff_layout } : {}),
  };
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
  layout: DiffLayout;
  setLayout: (layout: DiffLayout) => void;
  hideHistory: boolean;
  setHideHistory: (hide: boolean) => void;
}

export function useTimelinePreferences(): TimelinePreferences {
  const queryClient = useQueryClient();
  const state = useQuery(meStateQuery);
  const stored = readStored(state.data);
  const save = useMutation({
    mutationFn: (change: Stored) => setMeState('timeline', { ...readStored(queryClient.getQueryData(meStateKey)), ...change }),
    onMutate: (change) => {
      const before = queryClient.getQueryData<Record<string, unknown>>(meStateKey);
      queryClient.setQueryData<Record<string, unknown>>(meStateKey, { ...before, timeline: { ...readStored(before), ...change } });
      return { before };
    },
    onError: (_error, _change, context) => queryClient.setQueryData(meStateKey, context?.before),
  });
  const hideHistory = useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    () => hidden,
  );
  return {
    detail: stored.work_detail ?? DEFAULT_WORK_DETAIL,
    setDetail: useCallback((detail) => save.mutate({ work_detail: detail }), [save]),
    layout: stored.diff_layout ?? 'one',
    setLayout: useCallback((layout) => save.mutate({ diff_layout: layout }), [save]),
    hideHistory,
    setHideHistory: setHidden,
  };
}
