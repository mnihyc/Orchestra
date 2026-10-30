/** User live state; the socket client owns live updates.
 *
 * The person's own feed: the rows of their Inbox as the live connection keeps them, and the two
 * counters that travel with them (how many rows are unread, how many approvals wait for them). A row
 * is replaced only by one with a higher Inbox number; nothing is removed on the page's own initiative.
 *
 * The rows are held as the server lists them, newest first, by pages: the first page comes with the
 * feed, older ones when the Inbox asks for them. Rows found another way (the unread ones of a
 * distant page) are held too, but the newest-first order is only known as far as the pages reach, so
 * a row beyond that edge is shown by the filter that asked for it and by no other, until the pages
 * reach it. News that is no row (an approval was decided elsewhere, a route changed) arrives as
 * events, which whoever cares about one registers for.
 *
 * Components read through the hooks at the end. The writers are for the live connection and for the
 * acts of the person on their own rows.
 */
import { useMemo } from 'react';
import { create } from 'zustand';

import type { Notification } from '@/generated/api-types';

export interface Counters {
  /** How many rows of the Inbox are not read. */
  unread: number;
  /** How many approvals wait for a decision the person may give. */
  approvalsPending: number;
}

/** The place of a row in the newest-first order. */
export type Place = Pick<Notification, 'created_at' | 'id'>;

export interface UserState {
  /** Whether the Inbox was loaded whole at least once. */
  loaded: boolean;
  /** What the last attempt to load it failed with, while nothing newer has succeeded. */
  error: unknown;
  /** The Inbox number the rows are as of. */
  lastSeq: number;
  counters: Counters;
  /** Every row held, by id. */
  rows: ReadonlyMap<string, Notification>;
  /** The oldest row of the pages loaded one after the other from the newest; `null` while no page was loaded or the pages reached the end. */
  edge: Place | null;
  /** What to ask for the next page of that chain; `null` at the end. */
  nextCursor: string | null;
  /** Whether the pages reached the oldest row there is. */
  reachedEnd: boolean;
}

const INITIAL: UserState = {
  loaded: false,
  error: null,
  lastSeq: 0,
  counters: { unread: 0, approvalsPending: 0 },
  rows: new Map(),
  edge: null,
  nextCursor: null,
  reachedEnd: false,
};

export const useUserStore = create<UserState>(() => INITIAL);

/** Newest first, as the server lists the Inbox: by the moment written (a chat renews it), then by the identifier. */
export function byNewest(a: Place, b: Place): number {
  const delta = Date.parse(b.created_at) - Date.parse(a.created_at);
  if (delta !== 0 && !Number.isNaN(delta)) return delta;
  return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
}

/** Whether a row lies within the pages loaded from the newest, so that nothing between it and the newest is missing. */
export function withinPages(state: Pick<UserState, 'edge' | 'reachedEnd'>, row: Place): boolean {
  return state.reachedEnd || state.edge === null || byNewest(row, state.edge) <= 0;
}

/** The rows of a page merged into those held: each replaces the one held only when it is newer. */
function merged(held: ReadonlyMap<string, Notification>, rows: readonly Notification[], keep: (row: Notification) => boolean): Map<string, Notification> | null {
  let next: Map<string, Notification> | null = null;
  for (const row of rows) {
    const had = (next ?? held).get(row.id);
    if (had !== undefined) {
      if (had.inbox_seq >= row.inbox_seq) continue;
    } else if (!keep(row)) {
      continue;
    }
    next ??= new Map(held);
    next.set(row.id, row);
  }
  return next;
}

const last = <T>(list: readonly T[]): T | undefined => list[list.length - 1];

function set(change: (state: UserState) => Partial<UserState> | UserState): void {
  useUserStore.setState((state) => {
    const next = change(state);
    return next === state ? state : next;
  });
}

/** The writers. */
export const userWriter = {
  /** Takes a whole Inbox in place of what was held: the first page, the counters, and where the feed stands. */
  reset(rows: readonly Notification[], counters: Counters, seq: number, nextCursor: string | null): void {
    const sorted = [...rows].sort(byNewest);
    set(() => ({
      loaded: true,
      error: null,
      lastSeq: seq,
      counters,
      rows: new Map(sorted.map((row) => [row.id, row])),
      edge: nextCursor === null ? null : (last(sorted) ?? null),
      nextCursor,
      reachedEnd: nextCursor === null,
    }));
  },

  /** Takes rows that changed and the counters of that moment. A row the pages have not reached is not held: it comes with its page. */
  apply(rows: readonly Notification[], counters: Counters | null, seq: number): void {
    set((state) => {
      const next = merged(state.rows, rows, (row) => withinPages(state, row));
      if (next === null && counters === null && seq <= state.lastSeq) return state;
      return {
        ...(next === null ? {} : { rows: next }),
        ...(counters === null ? {} : { counters }),
        lastSeq: Math.max(state.lastSeq, seq),
        error: null,
      };
    });
  },

  /** Takes the next page of the newest-first chain. */
  older(rows: readonly Notification[], nextCursor: string | null): void {
    set((state) => {
      const next = merged(state.rows, rows, () => true) ?? state.rows;
      const sorted = [...rows].sort(byNewest);
      return {
        rows: next,
        nextCursor,
        reachedEnd: nextCursor === null,
        edge: nextCursor === null ? null : (last(sorted) ?? state.edge),
      };
    });
  },

  /** Takes rows that were found without the chain (the unread ones of a distant page). */
  found(rows: readonly Notification[]): void {
    set((state) => {
      const next = merged(state.rows, rows, () => true);
      return next === null ? state : { rows: next };
    });
  },

  /**
   * Marks rows read here, at once, as the person's own easily undone act. Answers with the rows as
   * they were, so that a refusal can put them back.
   */
  markRead(ids: readonly string[], at: string): Notification[] {
    const before: Notification[] = [];
    set((state) => {
      let next: Map<string, Notification> | null = null;
      for (const id of ids) {
        const row = state.rows.get(id);
        if (row?.read_at !== null) continue;
        before.push(row);
        next ??= new Map(state.rows);
        next.set(id, { ...row, read_at: at });
      }
      if (next === null) return state;
      return { rows: next, counters: { ...state.counters, unread: Math.max(0, state.counters.unread - before.length) } };
    });
    return before;
  },

  /** Puts rows back as they were, after the server refused to mark them. */
  unmarkRead(before: readonly Notification[]): void {
    set((state) => {
      if (before.length === 0) return state;
      const next = new Map(state.rows);
      let restored = 0;
      for (const row of before) {
        const now = next.get(row.id);
        // Only a row still marked by this act is put back: the server may have said better since.
        if (now?.inbox_seq === row.inbox_seq && now.read_at !== null) {
          next.set(row.id, row);
          restored += 1;
        }
      }
      return { rows: next, counters: { ...state.counters, unread: state.counters.unread + restored } };
    });
  },

  /** Changes the counters, the part of them that is given. */
  counters(counters: Partial<Counters>): void {
    set((state) => ({ counters: { ...state.counters, ...counters } }));
  },

  /** Records that the Inbox could not be loaded. What is held stays, to be read greyed. */
  failed(error: unknown): void {
    set(() => ({ error }));
  },

  /** Forgets everything: whoever was signed in has left. */
  clear(): void {
    useUserStore.setState(INITIAL, true);
  },
};

// ---------------------------------------------------------------- news that is no row

/** What a handler of the person's news is given: the `data` of the frame. */
export type UserEventHandler = (data: Record<string, unknown>) => void;

const handlers = new Map<string, Set<UserEventHandler>>();

/**
 * Asks to be told when the server sends news of one kind on the person's feed (`what`: `approvals`,
 * `route`, `chat_state`, `favorites`, ...). The pseudo-kind `connected` is told after every connection
 * is made anew, since news is never replayed and whoever depends on it loads its state again.
 * Answers how to stop asking.
 */
export function onUserEvent(what: string, handler: UserEventHandler): () => void {
  let registered = handlers.get(what);
  if (registered === undefined) {
    registered = new Set();
    handlers.set(what, registered);
  }
  const mine = registered;
  mine.add(handler);
  return () => void mine.delete(handler);
}

/** Tells the handlers of a kind. Only the live connection calls it. */
export function tellUserEvent(what: string, data: Record<string, unknown>): void {
  for (const handler of handlers.get(what) ?? []) handler(data);
}

// ---------------------------------------------------------------- reading

/** The two counters, for the rail and the left bar. */
export function useCounters(): Counters & { loaded: boolean } {
  const unread = useUserStore((state) => state.counters.unread);
  const approvalsPending = useUserStore((state) => state.counters.approvalsPending);
  const loaded = useUserStore((state) => state.loaded);
  return { unread, approvalsPending, loaded };
}

/** Every row held, newest first. */
export function useHeldRows(): Notification[] {
  const rows = useUserStore((state) => state.rows);
  return useMemo(() => [...rows.values()].sort(byNewest), [rows]);
}
