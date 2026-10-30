/** The tips a person closed for good and the tutorials they finished, which follow them.
 *
 * What a person closed is part of the state the interface keeps for them on the server (`help` in what
 * `GET /me/state` answers), so a tip closed on one device is closed on the others. Where the server does not
 * answer for that state, the device keeps it, for this person alone, and says nothing: a tip that comes
 * back on another device is a small thing, and it is not an error.
 */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useCallback, useMemo, useRef, useState } from 'react';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { meStateKey, meStateQuery, setMeState } from '@/api/personal';

/** What is kept: the tips that were closed, and the tutorials that were finished. */
export interface HelpState {
  [key: string]: unknown;
  closed: string[];
  finished: string[];
  progress: Record<string, number>;
}

const EMPTY: HelpState = { closed: [], finished: [], progress: {} };
const KEY = 'help';

const texts = (value: unknown): string[] => (Array.isArray(value) ? value.filter((entry): entry is string => typeof entry === 'string') : []);

/** Reads the state from what the server holds under `help`; anything that is not as it should be is as good as absent. */
export function readHelpState(value: unknown): HelpState {
  if (typeof value !== 'object' || value === null) return EMPTY;
  const held = value as Record<string, unknown>;
  const progress = typeof held.progress === 'object' && held.progress !== null && !Array.isArray(held.progress)
    ? Object.fromEntries(Object.entries(held.progress).filter((entry): entry is [string, number] => typeof entry[1] === 'number' && Number.isSafeInteger(entry[1]) && entry[1] >= 0)) : {};
  return { ...held, closed: texts(held.closed), finished: texts(held.finished), progress };
}

const storageKey = (userId: string) => `orch.${userId}.help`;

function readDevice(userId: string | null): HelpState {
  if (userId === null) return EMPTY;
  try {
    return readHelpState(JSON.parse(localStorage.getItem(storageKey(userId)) ?? 'null'));
  } catch {
    return EMPTY;
  }
}

function writeDevice(userId: string | null, state: HelpState): void {
  if (userId === null) return;
  try {
    localStorage.setItem(storageKey(userId), JSON.stringify(state));
  } catch {
    // A device that keeps nothing still closes the tip for as long as the page is open.
  }
}

export interface Dismissed {
  /** Whether a tip was closed. */
  isClosed: (id: string) => boolean;
  /** Whether a tutorial was finished. */
  isFinished: (id: string) => boolean;
  close: (id: string) => void;
  finish: (id: string) => void;
  stepOf: (id: string) => number;
  keepStep: (id: string, step: number) => void;
  /** Brings every tip back. */
  reopenAll: () => void;
}

export function useDismissed(): Dismissed {
  const queryClient = useQueryClient();
  const userId = useStore(client.session, (state) => state.userId);
  const state = useQuery({ ...meStateQuery, enabled: userId !== null, retry: false });
  // What this page changed and the server has not yet confirmed, or the device's own where the server holds none.
  const [local, setLocal] = useState<{ userId: string | null; value: HelpState; edit: number } | null>(null);
  const edit = useRef(0);
  const held = useMemo<HelpState>(() => {
    if (local !== null && local.userId === userId) return local.value;
    if (state.data !== undefined) return readHelpState(state.data[KEY]);
    return readDevice(userId);
  }, [local, state.data, userId]);

  const save = useMutation({
    // Serialize local changes and read the complete server key before changing one part.
    scope: { id: `help:${userId ?? 'signed-out'}` },
    mutationFn: async (change: { userId: string; next: HelpState; edit: number; make: (before: HelpState) => HelpState }): Promise<HelpState> => {
      if (client.session.getState().userId !== change.userId) return change.next;
      let before: HelpState;
      try {
        const current = await queryClient.query({ ...meStateQuery, staleTime: 0 });
        before = readHelpState(current[KEY]);
      } catch {
        before = change.next;
      }
      const next = change.make(before);
      if (client.session.getState().userId !== change.userId) return next;
      await setMeState(KEY, next);
      return next;
    },
    onSuccess: (next, change) => {
      if (client.session.getState().userId !== change.userId) return;
      // What the server holds now is what the next read of the state shows.
      queryClient.setQueryData<Record<string, unknown>>(meStateKey, (before) => ({ ...(before ?? {}), [KEY]: next }));
      if (edit.current === change.edit) writeDevice(change.userId, next);
      setLocal((before) => before?.userId === change.userId && before.edit === change.edit ? null : before);
    },
  });

  const change = useCallback(
    (make: (before: HelpState) => HelpState) => {
      if (userId === null) return;
      const next = make(held);
      const revision = ++edit.current;
      setLocal({ userId, value: next, edit: revision });
      writeDevice(userId, next);
      save.mutate({ userId, next, edit: revision, make });
    },
    [held, userId, save],
  );

  return {
    isClosed: (id) => held.closed.includes(id),
    isFinished: (id) => held.finished.includes(id),
    close: (id) => change((before) => (before.closed.includes(id) ? before : { ...before, closed: [...before.closed, id] })),
    finish: (id) => change((before) => ({ ...before, finished: before.finished.includes(id) ? before.finished : [...before.finished, id], progress: Object.fromEntries(Object.entries(before.progress).filter(([key]) => key !== id)) })),
    stepOf: (id) => held.progress[id] ?? 0,
    keepStep: (id, step) => { if (Number.isSafeInteger(step) && step >= 0) change((before) => ({ ...before, progress: { ...before.progress, [id]: step } })); },
    reopenAll: () => change((before) => ({ ...before, closed: [] })),
  };
}
