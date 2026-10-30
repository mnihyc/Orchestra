/** Document live state; the socket client owns live updates.
 *
 * What the feed of a document delivered: the document itself, its blocks as the server last
 * confirmed each (a removed block stays as a row that says so), its discussions with their comments,
 * who has it open and who is writing in which block. A row replaces the one held only when it is
 * newer. The editor draws from here and from one thing more, the changes its person made that the
 * server has not answered yet, which are not kept here (`features/documents/sync.ts`).
 *
 * Components read through the hooks at the end. The writers are for the live connection and for the
 * answers of the person's own requests.
 */
import { create } from 'zustand';

import type { Document, DocumentBlock, DocumentDiscussion, DocumentRef, PresenceUser, UserRef } from '@/generated/api-types';

/** Who is writing in a block, shown for this long after their last sign of it. */
export const WRITING_SHOWN_MS = 6_000;
/** The live preview window; older discussions are read through the paged list. */
export const DOCUMENT_DISCUSSION_WINDOW = 200;

function discussionWindow(rows: Iterable<DocumentDiscussion>): Map<string, DocumentDiscussion> {
  const window = new Map<string, DocumentDiscussion>();
  for (const row of rows) {
    window.delete(row.id);
    window.set(row.id, { ...row, comments: row.comments.slice(-50) });
    if (window.size > DOCUMENT_DISCUSSION_WINDOW) {
      const oldest = window.keys().next().value;
      if (oldest !== undefined) window.delete(oldest);
    }
  }
  return window;
}

export interface Writing {
  user: UserRef;
  block: string;
  /** When the last sign of it arrived, on this device's clock. */
  at: number;
}

export interface DocumentState {
  /** The document as the last whole load had it, with its live fields brought up to date by the rows that came since. */
  document: Document | null;
  /** The document sequence the rows are as of. */
  lastSeq: number;
  loaded: boolean;
  /** Every block the feed delivered, removed ones among them, by identifier. */
  blocks: ReadonlyMap<string, DocumentBlock>;
  discussions: ReadonlyMap<string, DocumentDiscussion>;
  presence: readonly PresenceUser[];
  writing: ReadonlyMap<string, Writing>;
  /** What the last attempt to load it failed with, while nothing newer has succeeded. */
  error: unknown;
  /** The server ended the feed: the reader may no longer read it, or it is gone. */
  ended: 'revoked' | 'deleted' | null;
}

const EMPTY: DocumentState = {
  document: null,
  lastSeq: 0,
  loaded: false,
  blocks: new Map(),
  discussions: new Map(),
  presence: [],
  writing: new Map(),
  error: null,
  ended: null,
};

export const useDocumentStore = create<{ byId: ReadonlyMap<string, DocumentState> }>(() => ({ byId: new Map() }));

function update(id: string, change: (state: DocumentState) => DocumentState): void {
  useDocumentStore.setState((store) => {
    const before = store.byId.get(id) ?? EMPTY;
    const after = change(before);
    if (after === before) return store;
    const byId = new Map(store.byId);
    byId.set(id, after);
    return { byId };
  });
}

/** What was delivered, as the feed and the HTTP request both say it. */
export interface DocumentRows {
  document?: Document | DocumentRef | null | undefined;
  blocks?: readonly DocumentBlock[] | null | undefined;
  discussions?: readonly DocumentDiscussion[] | null | undefined;
}

/** Who wants to know of blocks that changed, to lay them over what the editor holds. */
export type RowsListener = (rows: readonly DocumentBlock[], before: ReadonlyMap<string, DocumentBlock>, reset: boolean) => void;
const listeners = new Map<string, Set<RowsListener>>();

/** Hears of every block row that arrives for a document and is newer than what was held. What is returned stops the listening. */
export function listenToRows(id: string, listener: RowsListener): () => void {
  let set = listeners.get(id);
  if (set === undefined) {
    set = new Set();
    listeners.set(id, set);
  }
  set.add(listener);
  return () => {
    set.delete(listener);
  };
}

function tell(id: string, rows: readonly DocumentBlock[], before: ReadonlyMap<string, DocumentBlock>, reset: boolean): void {
  for (const listener of listeners.get(id) ?? []) listener(rows, before, reset);
}

/** Brings a held document up to date by a reference to it: its live fields change, what only a whole load carries stays. */
function mergeDocument(held: Document | null, next: Document | DocumentRef | null | undefined): Document | null {
  if (next === null || next === undefined) return held;
  if (held === null) return 'access' in next ? next : null;
  if (next.seq < held.seq) return held;
  return { ...held, ...next };
}

/** The writers. Only the live connection and the answers of the person's own requests call them. */
export const documentWriter = {
  /** Takes a whole load in place of what was held. */
  reset(id: string, rows: DocumentRows, seq: number): void {
    const blocks = new Map((rows.blocks ?? []).map((row) => [row.id, row]));
    const before = documentStateNow(id).blocks;
    update(id, (state) => ({
      ...state,
      document: 'access' in (rows.document ?? {}) ? (rows.document as Document) : (state.document ?? null),
      lastSeq: seq,
      loaded: true,
      blocks,
      discussions: discussionWindow(rows.discussions ?? []),
      error: null,
      ended: null,
    }));
    tell(id, [...blocks.values()], before, true);
  },

  /** Takes rows that changed: each replaces the row held only when it is newer. */
  apply(id: string, rows: DocumentRows, seq: number): void {
    const fresh: DocumentBlock[] = [];
    const before = documentStateNow(id).blocks;
    update(id, (state) => {
      let blocks: Map<string, DocumentBlock> | null = null;
      for (const row of rows.blocks ?? []) {
        const held = state.blocks.get(row.id);
        if (held !== undefined && held.seq_updated >= row.seq_updated) continue;
        blocks ??= new Map(state.blocks);
        blocks.set(row.id, row);
        fresh.push(row);
      }
      let discussions: Map<string, DocumentDiscussion> | null = null;
      for (const row of rows.discussions ?? []) {
        const held = state.discussions.get(row.id);
        if (held !== undefined && held.seq_updated >= row.seq_updated) continue;
        discussions ??= new Map(state.discussions);
        discussions.delete(row.id);
        discussions.set(row.id, { ...row, comments: row.comments.slice(-50) });
        if (discussions.size > DOCUMENT_DISCUSSION_WINDOW) {
          const oldest = discussions.keys().next().value;
          if (oldest !== undefined) discussions.delete(oldest);
        }
      }
      const document = mergeDocument(state.document, rows.document);
      if (blocks === null && discussions === null && document === state.document && seq <= state.lastSeq) return state;
      return {
        ...state,
        document,
        blocks: blocks ?? state.blocks,
        discussions: discussions ?? state.discussions,
        lastSeq: Math.max(state.lastSeq, seq),
        error: null,
      };
    });
    if (fresh.length > 0) tell(id, fresh, before, false);
  },

  /** Who has the document open, as the server last said. */
  presence(id: string, users: readonly PresenceUser[]): void {
    update(id, (state) => ({ ...state, presence: users }));
  },

  /** A sign that somebody is writing in a block. */
  typing(id: string, user: UserRef, block: string, now: number): void {
    update(id, (state) => {
      const writing = new Map(state.writing);
      writing.set(user.id, { user, block, at: now });
      return { ...state, writing };
    });
  },

  /** Forgets the signs of writing that are older than they are shown for. */
  quiet(id: string, now: number): void {
    update(id, (state) => {
      let writing: Map<string, Writing> | null = null;
      for (const [who, sign] of state.writing) {
        if (now - sign.at < WRITING_SHOWN_MS) continue;
        writing ??= new Map(state.writing);
        writing.delete(who);
      }
      return writing === null ? state : { ...state, writing };
    });
  },

  /** Records that the document could not be loaded. What is held stays, to be read greyed. */
  failed(id: string, error: unknown): void {
    update(id, (state) => ({ ...state, error }));
  },

  /** The server ended the feed. */
  ended(id: string, reason: 'revoked' | 'deleted'): void {
    update(id, (state) => ({ ...state, ended: reason }));
  },

  /** Forgets a document nobody shows any more. */
  evict(id: string): void {
    useDocumentStore.setState((store) => {
      if (!store.byId.has(id)) return store;
      const byId = new Map(store.byId);
      byId.delete(id);
      return { byId };
    });
  },

  /** Forgets everything: whoever was signed in has left. */
  clear(): void {
    useDocumentStore.setState({ byId: new Map() });
    listeners.clear();
  },
};

// ---------------------------------------------------------------- reading

/** A document as far as it has arrived. */
export function useDocumentState(id: string | null): DocumentState {
  return useDocumentStore((store) => (id === null ? EMPTY : (store.byId.get(id) ?? EMPTY)));
}

/** The state of a document without watching it: for code that reads once, at the moment it acts. */
export function documentStateNow(id: string): DocumentState {
  return useDocumentStore.getState().byId.get(id) ?? EMPTY;
}
