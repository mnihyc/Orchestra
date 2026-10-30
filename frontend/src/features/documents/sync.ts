/** What is typed is sent as batches of operations, one at a time and in order, shown as "Saving…" until answered; what others write is laid over the page; and the change that came second is kept beside the first.
 *
 * Typing never waits for anything: the editor changes at once, and this module notices that it did.
 * A line is sent about a second after the last key, and a block added, moved, turned into another or
 * removed at once. One batch is in flight at a time and keeps its `client_id` until it is answered,
 * so that a second attempt after a lost answer changes nothing twice; what is typed meanwhile goes in
 * the next. A block that nobody changed is never sent, whatever the editor did to its own state when it
 * opened the page.
 */
import type { Editor } from '@tiptap/core';
import type { Node as PmNode } from '@tiptap/pm/model';
import type { Transaction } from '@tiptap/pm/state';
import { AddMarkStep, RemoveMarkStep, ReplaceStep } from '@tiptap/pm/transform';
import { createStore, type StoreApi } from 'zustand/vanilla';

import { postOps, staleBlocks } from '@/api/documents';
import { buildTree, inBatches, planOps, sameBlock, type Tree } from '@/features/documents/blocks';
import { containerAt, removeBlock } from '@/features/documents/commands';
import { applyRows, containersOf, findContainer, REMOTE, replaceAll, updateContent, WHOLESALE_ROWS } from '@/features/documents/remote';
import { docToDrafts, draftOf, newId } from '@/features/documents/schema';
import type { Author, DocumentBlock, DocumentOp } from '@/generated/api-types';
import { NetworkError, retryPause } from '@/lib/requests';
import { sendWriting, socket } from '@/realtime/live';
import { documentStateNow, documentWriter, listenToRows } from '@/realtime/stores/document';

/** A line is sent this long after the last key in it; the signs of writing go no oftener than every few seconds. */
export const SEND_AFTER_MS = 1_000;
const SIGN_EVERY_MS = 2_500;
/** The pause before a batch is sent again at most. */
const RETRY_MOST_MS = 15_000;

export type SavePhase = 'saved' | 'saving' | 'offline' | 'failed';

export interface SaveState {
  phase: SavePhase;
  /** What the last attempt failed with, while it is still the person's to see. */
  error: unknown;
}

/** A block the person wrote that was kept beside another, because that was changed or removed meanwhile. */
export interface Kept {
  id: string;
  /** The block it was written on, when that is still there. */
  of: string | null;
  note: 'changed' | 'removed';
  /** Who changed or removed it. */
  by: Author | null;
}

export interface Sync {
  save: StoreApi<SaveState>;
  kept: StoreApi<{ byId: ReadonlyMap<string, Kept> }>;
  /** Sends what is unsent now. */
  flush: () => void;
  /** Whether something the person wrote has not been answered. */
  hasUnsent: () => boolean;
  /** Leaves the kept block as a block of the page. */
  keepBoth: (id: string) => void;
  /** Takes the kept block away. */
  discardMine: (id: string) => void;
  /** Puts the person's text where the other's stood, and takes the kept block away. */
  replaceTheirs: (id: string) => void;
  /** Stops listening and sending. */
  destroy: () => void;
}

/** The syncs of the editors that are open: whatever draws from a page's sync finds it by the editor. */
const syncs = new WeakMap<Editor, Sync>();

/** The sync tied to an editor, or `null` while there is none. */
export function syncOf(editor: Editor): Sync | null {
  return syncs.get(editor) ?? null;
}

export interface SyncOptions {
  editor: Editor;
  docId: string;
  /** How a batch is sent: the request itself unless a test says otherwise. */
  send?: typeof postOps;
  sendAfterMs?: number;
}

/** Whether a transaction changed more than the words of one line: a block came, went, moved or became another. */
function structural(steps: readonly unknown[], docs: readonly PmNode[]): boolean {
  return steps.some((step, index) => {
    if (step instanceof AddMarkStep || step instanceof RemoveMarkStep) return false;
    if (!(step instanceof ReplaceStep)) return true;
    const before = docs[index];
    if (before === undefined) return true;
    const first = step.slice.content.firstChild;
    if (first !== null && !first.isInline) return true;
    return !before.resolve(step.from).sameParent(before.resolve(step.to));
  });
}

/** Ties the page of blocks in an editor to the document on the server. */
export function createSync(options: SyncOptions): Sync {
  const { editor, docId } = options;
  const send = options.send ?? postOps;
  const sendAfter = options.sendAfterMs ?? SEND_AFTER_MS;
  const save = createStore<SaveState>(() => ({ phase: 'saved', error: null }));
  const kept = createStore<{ byId: ReadonlyMap<string, Kept> }>(() => ({ byId: new Map() }));

  /** The row a container was last found to say what, so that a block nobody touched is not compared again. */
  const checked = new WeakMap<PmNode, number>();
  /** The stamp of the row each block's text in the editor stands on: what an update names as the one it was written on. */
  const base = new Map<string, number>();
  let inFlight: { clientId: string; ops: DocumentOp[]; ids: Set<string> } | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let failures = 0;
  let destroyed = false;
  /** Whether this sync was ended: asked through a function, since it changes while a request is waiting for its answer. */
  const isGone = (): boolean => destroyed;
  let treeCache: { blocks: ReadonlyMap<string, DocumentBlock>; tree: Tree } | null = null;
  let lastSign = { block: '', at: 0 };

  const confirmed = (): Tree => {
    const blocks = documentStateNow(docId).blocks;
    if (treeCache?.blocks !== blocks) treeCache = { blocks, tree: buildTree(blocks.values()) };
    return treeCache.tree;
  };

  /** What the server holds is what the page shows, at the start: nothing is sent for a page nobody changed. */
  function trustPage(): void {
    const tree = confirmed();
    for (const [id, found] of containersOf(editor.state.doc)) {
      const row = tree.rows.get(id);
      if (row === undefined) continue;
      checked.set(found.node, row.seq_updated);
      base.set(id, row.seq_updated);
    }
  }

  function isDirty(id: string, node: PmNode, row: DocumentBlock | undefined): boolean {
    if (inFlight?.ids.has(id) === true) return true;
    if (row === undefined) return true;
    if (checked.get(node) === row.seq_updated) return false;
    const clean = sameBlock(draftOf(node, null), row);
    if (clean) checked.set(node, row.seq_updated);
    return !clean;
  }

  const set = (phase: SavePhase, error: unknown = null) => {
    const now = save.getState();
    if (now.phase !== phase || now.error !== error) save.setState({ phase, error });
  };

  // ---------------------------------------------------------------- sending

  function plan() {
    const tree = confirmed();
    const unchanged = new Set<string>();
    for (const [id, found] of containersOf(editor.state.doc)) {
      const row = tree.rows.get(id);
      if (row !== undefined && checked.get(found.node) === row.seq_updated) unchanged.add(id);
    }
    const result = planOps(tree, docToDrafts(editor.state.doc), { unchanged, baseOf: (id) => base.get(id) });
    // What was compared and found equal is known to be so from now on.
    const doc = editor.state.doc;
    for (const [id, found] of containersOf(doc)) {
      const row = tree.rows.get(id);
      if (row === undefined || result.touched.has(id) || unchanged.has(id)) continue;
      checked.set(found.node, row.seq_updated);
      base.set(id, row.seq_updated);
    }
    return result;
  }

  async function attempt(): Promise<void> {
    const batch = inFlight;
    if (batch === null || isGone()) return;
    try {
      const reply = await send(docId, batch.clientId, batch.ops);
      if (isGone()) return;
      failures = 0;
      // The answer is applied as a row of the feed is, and the blocks it names now stand on the stamp it gave them.
      documentWriter.apply(docId, { document: reply.document, blocks: reply.blocks }, reply.seq);
      for (const row of reply.blocks) if (!row.removed) base.set(row.id, row.seq_updated);
      inFlight = null;
      flush();
    } catch (error) {
      if (isGone()) return;
      const stale = staleBlocks(error);
      if (stale !== null) {
        inFlight = null;
        keepBeside(stale.current);
        flush();
        return;
      }
      if (error instanceof NetworkError) {
        // The line is down or slow: the same batch goes again, and the person goes on writing.
        failures += 1;
        set('offline', error);
        const pause = Math.min(RETRY_MOST_MS, retryPause(failures - 1));
        clearTimeout(retryTimer);
        retryTimer = setTimeout(() => void attempt(), pause);
        return;
      }
      // The server said no for a reason that sending again will not change; what is written stays on the page.
      inFlight = null;
      set('failed', error);
    }
  }

  function flush(): void {
    clearTimeout(timer);
    timer = undefined;
    if (isGone() || !editor.isEditable) return;
    // One batch at a time: what is written meanwhile is looked at when this one is answered.
    if (inFlight !== null) return;
    const result = plan();
    if (result.ops.length === 0) {
      set('saved');
      return;
    }
    const ops = inBatches(result.ops)[0] ?? [];
    inFlight = { clientId: newId(), ops, ids: new Set(ops.flatMap((op) => (op.op === 'insert' ? [op.block.id] : [op.id]))) };
    set('saving');
    void attempt();
  }

  const unsubscribeSocket = socket.status.subscribe((now, before) => {
    // The line is back: a batch that was waiting for it is sent at once.
    if (now.phase === 'ready' && before.phase !== 'ready' && save.getState().phase === 'offline') {
      clearTimeout(retryTimer);
      void attempt();
    }
  });
  const onOnline = () => {
    if (save.getState().phase === 'offline') {
      clearTimeout(retryTimer);
      void attempt();
    }
  };
  if (typeof window !== 'undefined') window.addEventListener('online', onOnline);

  // ---------------------------------------------------------------- conflicts

  /** A batch ran into blocks that changed meanwhile: what they say stands, and what the person wrote is kept beside it. */
  function keepBeside(current: readonly DocumentBlock[]): void {
    // The person's blocks among them, judged against the rows they were written on, before those rows are replaced.
    const mine = new Map<string, PmNode>();
    for (const row of current) {
      const found = findContainer(editor.state.doc, row.id);
      if (found !== null && isDirty(row.id, found.node, documentStateNow(docId).blocks.get(row.id))) mine.set(row.id, found.node);
    }
    // The others are laid over the page as any rows are, and the blocks the person changed are left to this.
    documentWriter.apply(docId, { blocks: current }, Math.max(0, ...current.map((row) => row.seq_updated)));
    for (const row of current) {
      const node = mine.get(row.id);
      if (node === undefined) continue;
      const fresh = newId();
      const tr = editor.state.tr;
      tr.setMeta('addToHistory', false);
      const found = findContainer(tr.doc, row.id);
      if (found === null) continue;
      if (row.removed) {
        // It was removed while it was being written in: what was written stays, as a block of its own, where it stood.
        tr.setNodeMarkup(found.pos, undefined, { ...node.attrs, id: fresh });
        base.delete(row.id);
      } else {
        // Their text stands; the person's is kept as a block directly after it.
        updateContent(tr, found, row);
        const there = findContainer(tr.doc, row.id);
        if (there === null) continue;
        const copy = node.type.create({ ...node.attrs, id: fresh }, node.child(0));
        tr.insert(there.pos + there.node.nodeSize, copy);
        base.set(row.id, row.seq_updated);
      }
      editor.view.dispatch(tr);
      if (!row.removed) {
        const settled = findContainer(editor.state.doc, row.id);
        if (settled !== null) checked.set(settled.node, row.seq_updated);
      }
      const entry: Kept = { id: fresh, of: row.removed ? null : row.id, note: row.removed ? 'removed' : 'changed', by: row.updated_by };
      kept.setState((state) => ({ byId: new Map([...state.byId, [fresh, entry]]) }));
    }
  }

  function keepBoth(id: string): void {
    kept.setState((state) => {
      const byId = new Map(state.byId);
      byId.delete(id);
      return { byId };
    });
  }

  function discardMine(id: string): void {
    const found = findContainer(editor.state.doc, id);
    if (found !== null) {
      const tr = removeBlock(editor.state, found.pos);
      if (tr !== null) editor.view.dispatch(tr);
    }
    keepBoth(id);
    scheduleSend(true);
  }

  function replaceTheirs(id: string): void {
    const entry = kept.getState().byId.get(id);
    const mine = findContainer(editor.state.doc, id);
    const original = entry?.of ?? null;
    const theirs = original === null ? null : findContainer(editor.state.doc, original);
    if (mine !== null && theirs !== null && mine.node.type === theirs.node.type) {
      // Their block takes the person's words and the kept block goes; the later place first, so that the earlier one stays where it is.
      const tr = editor.state.tr;
      tr.delete(mine.pos, mine.pos + mine.node.nodeSize);
      tr.replaceWith(theirs.pos + 1, theirs.pos + 1 + theirs.node.child(0).nodeSize, mine.node.child(0));
      editor.view.dispatch(tr);
    }
    keepBoth(id);
    scheduleSend(true);
  }

  // ---------------------------------------------------------------- watching the editor

  function scheduleSend(now: boolean): void {
    if (destroyed || !editor.isEditable) return;
    if (save.getState().phase === 'failed') set('saving');
    else if (save.getState().phase === 'saved') set('saving');
    clearTimeout(timer);
    timer = setTimeout(flush, now ? 0 : sendAfter);
  }

  const onTransaction = ({ transaction }: { transaction: Transaction }) => {
    if (!transaction.docChanged || transaction.getMeta(REMOTE) === true) return;
    scheduleSend(structural(transaction.steps, transaction.docs));
    const at = containerAt(editor.state.doc, editor.state.selection.head);
    if (at !== null) {
      const block = String(at.node.attrs.id);
      const t = Date.now();
      if (block !== lastSign.block || t - lastSign.at > SIGN_EVERY_MS) {
        lastSign = { block, at: t };
        sendWriting(docId, block);
      }
    }
  };
  editor.on('transaction', onTransaction);

  // ---------------------------------------------------------------- other people's work

  const stopListening = listenToRows(docId, (rows, before, reset) => {
    if (isGone()) return;
    const tree = confirmed();
    const context = { tree, before, isDirty: (id: string, node: PmNode) => isDirty(id, node, before.get(id)) };
    const unsent = inFlight !== null || save.getState().phase !== 'saved';
    let toApply = rows;
    if (reset) {
      // Whatever the page holds that the server no longer does goes, unless the person changed it.
      const gone: DocumentBlock[] = [];
      for (const [id, found] of containersOf(editor.state.doc)) {
        if (tree.rows.has(id) || isDirty(id, found.node, before.get(id))) continue;
        const old = before.get(id);
        if (old !== undefined) gone.push({ ...old, removed: true });
      }
      toApply = [...rows, ...gone];
    }
    if ((reset || rows.length > WHOLESALE_ROWS) && !unsent) {
      const anchor = containerAt(editor.state.doc, editor.state.selection.head);
      const offset = anchor === null ? 0 : editor.state.selection.head - anchor.pos;
      editor.view.dispatch(replaceAll(editor.state, tree));
      trustPage();
      if (anchor !== null) {
        const found = findContainer(editor.state.doc, String(anchor.node.attrs.id));
        if (found !== null) editor.commands.setTextSelection(Math.min(editor.state.doc.content.size, found.pos + offset));
      }
      return;
    }
    const tr = applyRows(editor.state, toApply, context);
    if (tr !== null) editor.view.dispatch(tr);
    // What now says what the server's row says is known to.
    for (const row of rows) {
      if (row.removed) continue;
      const found = findContainer(editor.state.doc, row.id);
      if (found === null || isDirty(row.id, found.node, before.get(row.id))) continue;
      if (sameBlock(draftOf(found.node, null), row)) {
        checked.set(found.node, row.seq_updated);
        base.set(row.id, row.seq_updated);
      }
    }
    // A change that waited for a stamp which has moved on is looked at again.
    if (save.getState().phase !== 'saved' && inFlight === null) scheduleSend(false);
  });

  trustPage();

  const api: Sync = {
    save,
    kept,
    flush: () => flush(),
    hasUnsent: () => inFlight !== null || save.getState().phase !== 'saved',
    keepBoth,
    discardMine,
    replaceTheirs,
    destroy() {
      destroyed = true;
      clearTimeout(timer);
      clearTimeout(retryTimer);
      editor.off('transaction', onTransaction);
      stopListening();
      unsubscribeSocket();
      if (typeof window !== 'undefined') window.removeEventListener('online', onOnline);
      syncs.delete(editor);
    },
  };
  syncs.set(editor, api);
  return api;
}
