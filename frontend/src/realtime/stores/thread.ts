/** Thread live state; the socket client owns live updates.
 *
 * Everything the screens of a thread read of it, as the live connection keeps it: the thread, its
 * conversation (the main stream in the order the items were made, and the streams of sub-agents),
 * its turns, what was said to its agent and where that stands, its segments, and who is looking.
 *
 * A row is replaced only by a newer one. Nothing is removed on the page's own initiative: what is
 * cancelled or deleted arrives as a row that says so. Growing text is joined into its item by
 * `deltas.ts`, a piece at a time, drawn once a frame. The answer to the person's own request is
 * taken like a row that came by the socket.
 *
 * Components read through the hooks at the end. The writers are for the live connection alone.
 */
import { currentThreadDisplay } from '@/lib/threadDisplay';
import type { ThreadDisplay } from '@/generated/api-types';
import { useStore } from 'zustand';
import { create } from 'zustand';

import type { Item, PresenceUser, Segment, Thread, ThreadContinuationProjection, ThreadInput, ThreadInputPage, ThreadPromptSummary, ThreadPromptVersion, Turn } from '@/generated/api-types';
import { matchesThreadCapture, matchesThreadProjection, newThreadReadScope, planThreadOpening, projectionFitsSelection, promptReceipt, sameThreadProjection, stampThreadReceipt, threadReceipt, type ThreadReadCapture, type ThreadReadScope, type ThreadReadRef, type ThreadReadTarget, sameThreadRead, threadReadCacheKey, threadReadTarget } from '@/realtime/threadScope';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { newOutputCounts, observeOutput, type OutputCounts } from '@/realtime/outputCounts';
import {
  type Applied,
  applyDelta,
  type Delta,
  HELD_PIECES_MOST,
  joinRow,
  mergeDeltas,
  appliedOf,
  dropCodePoints,
} from '@/realtime/deltas';

/** The rows of a thread as a sync answer or a frame carries them: each member there only when rows of its kind changed. */
export interface ThreadRows {
  /** Sequence of the admitted opening or socket cut, independent of optional thread-row changes. */
  seq?: number | undefined;
  projection?: ThreadContinuationProjection | null | undefined;
  prompt_summaries?: readonly ThreadPromptSummary[] | undefined;
  viewed_prompt?: ThreadPromptVersion | null | undefined;
  inputs_next_after?: string | null | undefined;
  inputs_queue_revision?: number | null | undefined;
  complete?: boolean | null | undefined;
  thread?: Thread | null | undefined;
  items?: readonly Item[] | null | undefined;
  turns?: readonly Turn[] | null | undefined;
  inputs?: readonly ThreadInput[] | null | undefined;
  segments?: readonly Segment[] | null | undefined;
}

export type Loaded = 'none' | 'tail' | 'window';

export interface QueuePageState {
  nextAfter: string | null;
  /** Native cursor of the first100, separate from the consumer's explicitly paged tail. */
  headNextAfter: string | null;
  revision: number | null;
  loading: boolean;
  error: unknown;
  restartRequired: boolean;
  /** Only actual explicitly loaded tail ids; no delivered-input history is fetched. */
  tail: ReadonlySet<string>;
  request: object | null;
}

/** Matches the native initial pending-input window, independently of explicit later pages. */
export const THREAD_INPUT_PAGE_SIZE = 100;

const EMPTY_QUEUE: QueuePageState = { nextAfter: null, headNextAfter: null, revision: null, loading: false,
  error: null, restartRequired: false, tail: new Set(), request: null };

export interface ThreadView {
  ref: ThreadReadRef | null;
  read: ThreadReadScope | null;
  promptSummaries: ReadonlyMap<string, ThreadPromptSummary>;
  /** Native retained status of the viewed edit input; absent is never inferred from Thread.job. */
  viewedPrompt: ThreadPromptVersion | null;
  /** The admitted cut that supplied viewedPrompt; never inferred from the thread metadata row. */
  viewedPromptSeq: number | null;
  queuePage: QueuePageState;
  thread: Thread | null;
  /** How much of the conversation is held: nothing, its newest part, or a window somewhere in it. */
  loaded: Loaded;
  /** Whether older items exist than the oldest held. */
  hasOlder: boolean;
  /** Whether newer items exist than the newest held; true only for a window that was read around an item. */
  hasNewer: boolean;
  items: ReadonlyMap<string, Item>;
  /** Maintained output counters; status clocks never visit the item map. */
  outputCounts: OutputCounts;
  /** The main stream, ascending by when each item was made. */
  order: readonly string[];
  /** The streams of sub-agents, each ascending. */
  streams: ReadonlyMap<string, readonly string[]>;
  turns: ReadonlyMap<string, Turn>;
  inputs: ReadonlyMap<string, ThreadInput>;
  segments: readonly Segment[];
  /** How far the growing text of each item has come. */
  applied: ReadonlyMap<string, Applied>;
  /** Pieces of growing text that wait for their item: it is not held yet, or is being read again. */
  held: ReadonlyMap<string, readonly Delta[]>;
  /** Items being read again because a piece began beyond what was held. */
  gaps: ReadonlySet<string>;
  presence: readonly PresenceUser[];
  /** How far the person had read when this view was first loaded: where the "New" line stands. */
  openedReadSeq: number | null;
  /** How far the thread had come at that moment: what arrives after it is seen as it arrives, and is never "new". */
  openedSeq: number | null;
  /** What the last attempt to load failed with, while nothing newer has succeeded. */
  error: unknown;
  /** The server ended the feed: the reader may no longer see the thread, or it is gone. */
  closed: 'revoked' | 'deleted' | null;
}

export const EMPTY_VIEW: ThreadView = {
  ref: null,
  read: null,
  promptSummaries: new Map(),
  viewedPrompt: null,
  viewedPromptSeq: null,
  queuePage: EMPTY_QUEUE,
  thread: null,
  loaded: 'none',
  hasOlder: false,
  hasNewer: false,
  items: new Map(),
  outputCounts: newOutputCounts(null),
  order: [],
  streams: new Map(),
  turns: new Map(),
  inputs: new Map(),
  segments: [],
  applied: new Map(),
  held: new Map(),
  gaps: new Set(),
  presence: [],
  openedReadSeq: null,
  openedSeq: null,
  error: null,
  closed: null,
};

// ---------------------------------------------------------------- the rules of merging, as pure functions

const MAIN_STREAM = 'main';

/** Orders two queue positions, which are decimal strings of any length. */
export function comparePositions(one: string, other: string): number {
  const [wholeOne = '0', fractionOne = ''] = one.split('.');
  const [wholeOther = '0', fractionOther = ''] = other.split('.');
  const left = wholeOne.replace(/^0+(?=\d)/, '');
  const right = wholeOther.replace(/^0+(?=\d)/, '');
  if (left.length !== right.length) return left.length - right.length;
  if (left !== right) return left < right ? -1 : 1;
  const width = Math.max(fractionOne.length, fractionOther.length);
  const a = fractionOne.padEnd(width, '0');
  const b = fractionOther.padEnd(width, '0');
  return a === b ? 0 : a < b ? -1 : 1;
}

/** Puts new ids into an ascending list, by when their items were made. */
function mergeOrder(order: readonly string[], added: readonly string[], items: ReadonlyMap<string, Item>): readonly string[] {
  if (added.length === 0) return order;
  const seqOf = (id: string) => items.get(id)?.seq_created ?? 0;
  const lastSeq = order.length === 0 ? -Infinity : seqOf(order[order.length - 1] ?? '');
  const sorted = [...added].sort((one, other) => seqOf(one) - seqOf(other));
  if (seqOf(sorted[0] ?? '') > lastSeq) return [...order, ...sorted];
  return [...order, ...sorted].sort((one, other) => seqOf(one) - seqOf(other));
}

/** Replaces rows that are older, and keeps the order and the sub-streams in step. */
export function applyRows(view: ThreadView, rows: ThreadRows): ThreadView {
  let next = view;
  const update = (change: Partial<ThreadView>) => {
    next = { ...next, ...change };
  };

  if (rows.projection !== undefined && rows.projection !== null && next.read !== null) {
    const before = next.read.projection;
    if (!projectionFitsSelection(next.read, rows.projection)
      || before !== null && !sameThreadProjection(before, rows.projection)) return view;
    // Explicit old views keep their history identity while learning today's execution selection.
    if (before === null || rows.projection.execution_revision >= before.execution_revision) {
      update({ read: { ...next.read, projection: rows.projection } });
    }
  }

  if (rows.viewed_prompt !== undefined && (rows.seq === undefined || next.viewedPromptSeq === null || rows.seq >= next.viewedPromptSeq)) {
    update({ viewedPrompt: rows.viewed_prompt, viewedPromptSeq: rows.seq ?? null });
  }

  if (rows.thread !== undefined && rows.thread !== null) {
    const held = next.thread;
    const newer = held === null || rows.thread.seq > held.seq || (rows.thread.seq === held.seq && rows.thread.revision > held.revision);
    if (newer) {
      const turnId = rows.thread.current_turn_id;
      const display = currentThreadDisplay(rows.thread.display, held?.display);
      update({
        thread: display === undefined || display === rows.thread.display ? rows.thread : { ...rows.thread, display },
        ...(next.outputCounts.turnId === turnId ? {} : { outputCounts: newOutputCounts(turnId) }),
      });
    }
  }

  if (rows.turns !== undefined && rows.turns !== null && rows.turns.length > 0) {
    let turns: Map<string, Turn> | null = null;
    for (const row of rows.turns) {
      const held = (turns ?? next.turns).get(row.id);
      if (held !== undefined && held.seq_updated >= row.seq_updated) continue;
      turns ??= new Map(next.turns);
      turns.set(row.id, row);
    }
    if (turns !== null) update({ turns });
  }

  if (rows.inputs !== undefined && rows.inputs !== null && rows.inputs.length > 0) {
    let inputs: Map<string, ThreadInput> | null = null;
    for (const row of rows.inputs) {
      const held = (inputs ?? next.inputs).get(row.id);
      if (held !== undefined && held.seq_updated >= row.seq_updated) continue;
      inputs ??= new Map(next.inputs);
      inputs.set(row.id, row);
    }
    if (inputs !== null) update({ inputs });
  }

  if (rows.segments !== undefined && rows.segments !== null && rows.segments.length > 0) {
    const bySegment = new Map(next.segments.map((segment) => [segment.id, segment]));
    let changed = false;
    for (const row of rows.segments) {
      const held = bySegment.get(row.id);
      if (held !== undefined && held.seq_updated >= row.seq_updated) continue;
      bySegment.set(row.id, row);
      changed = true;
    }
    if (changed) update({ segments: [...bySegment.values()].sort((one, other) => one.ordinal - other.ordinal) });
  }

  if (rows.items !== undefined && rows.items !== null && rows.items.length > 0) {
    next = applyItems(next, rows.items);
  }
  if (rows.inputs_queue_revision !== undefined && rows.inputs_queue_revision !== null
    && (next.queuePage.revision === null || rows.inputs_queue_revision >= next.queuePage.revision)) {
    const head = rows.inputs_next_after ?? null;
    const changed = next.queuePage.revision !== null && next.queuePage.revision !== rows.inputs_queue_revision;
    const hadTail = next.queuePage.tail.size > 0;
    const reset = changed || next.queuePage.headNextAfter !== head;
    const positioned = hadTail || next.queuePage.headNextAfter !== null || head !== null;
    if (reset && hadTail) {
      const referenced = referencedInputs(next);
      const inputs = new Map(next.inputs);
      for (const id of next.queuePage.tail) if (!referenced.has(id)) inputs.delete(id);
      update({ inputs });
    }
    update({ queuePage: { ...next.queuePage, revision: rows.inputs_queue_revision, headNextAfter: head,
      nextAfter: reset || !hadTail ? head : next.queuePage.nextAfter, ...(reset ? { tail: new Set<string>(),
        restartRequired: next.queuePage.restartRequired || next.queuePage.revision !== null && positioned,
        loading: false, error: null, request: null } : {}) } });
  }
  if (rows.prompt_summaries !== undefined && rows.prompt_summaries.length > 0) {
    const summaries = new Map(next.promptSummaries);
    for (const summary of rows.prompt_summaries) {
      summaries.set(summary.input_id, summary);
    }
    update({ promptSummaries: summaries });
  }
  return prunePromptMetadata(next);
}

function referencedInputs(view: ThreadView): Set<string> {
  const referenced = new Set<string>();
  for (const item of view.items.values()) if (item.kind === 'user_message' && item.payload.input_id !== null) referenced.add(item.payload.input_id);
  const current = view.thread?.current_turn_id;
  const input = current === undefined || current === null ? null : view.turns.get(current)?.input_id;
  if (typeof input === 'string') referenced.add(input);
  return referenced;
}

/** Summary/queue storage follows held history and explicit pages, not every changed input ever seen. */
function prunePromptMetadata(view: ThreadView): ThreadView {
  const referenced = referencedInputs(view);
  let next = view;
  if (view.read?.projection !== null && view.read?.projection !== undefined) {
    // Incremental rows are changes, not a replacement of the first100. A changed boundary is
    // visibly restart-required; unseen pending rows cannot grow an unrequested queue indefinitely.
    const open = [...view.inputs.values()].filter((row) => row.state === 'pending' || row.state === 'delivering' || row.state === 'failed');
    const head = open.filter((row) => !view.queuePage.tail.has(row.id)).sort((one,other) => comparePositions(one.position,other.position))
      .slice(0,THREAD_INPUT_PAGE_SIZE);
    const admitted = new Set(head.map((row) => row.id));
    for (const row of open) if (view.queuePage.tail.has(row.id)) admitted.add(row.id);
    const inputs = new Map([...view.inputs].filter(([id]) => referenced.has(id) || admitted.has(id)));
    if (inputs.size !== view.inputs.size) next = { ...next, inputs };
  }
  if (next.promptSummaries.size === 0) return next;
  for (const id of next.inputs.keys()) referenced.add(id);
  const kept = new Map([...next.promptSummaries].filter(([id]) => referenced.has(id)));
  return kept.size === next.promptSummaries.size ? next : { ...next, promptSummaries: kept };
}

function applyItems(view: ThreadView, rows: readonly Item[]): ThreadView {
  let items: Map<string, Item> | null = null;
  let applied: Map<string, Applied> | null = null;
  const addedMain: string[] = [];
  const addedSub = new Map<string, string[]>();
  const replayed: string[] = [];
  let outputCounts = view.outputCounts;

  for (const row of rows) {
    const held = (items ?? view.items).get(row.id);
    if (held !== undefined && held.seq_updated >= row.seq_updated) continue;
    items ??= new Map(view.items);
    applied ??= new Map(view.applied);
    let appended: string | undefined;
    if (held === undefined) {
      items.set(row.id, row);
      applied.set(row.id, appliedOf(row));
      // Out of a window that does not reach the newest items, a new item is held but not shown in the stream.
      if (row.stream === MAIN_STREAM) {
        if (!(view.loaded === 'window' && view.hasNewer)) addedMain.push(row.id);
      } else addedSub.set(row.stream, [...(addedSub.get(row.stream) ?? []), row.id]);
      if (view.held.has(row.id)) replayed.push(row.id);
    } else {
      const progress = applied.get(row.id) ?? appliedOf(held);
      const joined = joinRow(held, row, progress);
      // A persisted row behind the stream retains the existing body and its counters.
      if (joined.applied === progress) appended = '';
      items.set(row.id, joined.item);
      applied.set(row.id, joined.applied);
      if (view.held.has(row.id)) replayed.push(row.id);
    }
    outputCounts = observeOutput(outputCounts, held, items.get(row.id) ?? row, appended);
  }
  if (items === null || applied === null) return view;

  let order = view.order;
  if (addedMain.length > 0) order = mergeOrder(order, addedMain, items);
  let streams = view.streams;
  if (addedSub.size > 0) {
    const copy = new Map(streams);
    for (const [stream, ids] of addedSub) copy.set(stream, mergeOrder(copy.get(stream) ?? [], ids, items));
    streams = copy;
  }
  let next: ThreadView = { ...view, items, applied, order, streams, outputCounts };
  for (const row of rows) {
    const summary = promptReceipt(row);
    // Historical pages carry no independent metadata sequence. They seed a newly held prompt,
    // but cannot replace a newer summary already delivered by the ordered live/sync cut.
    if (summary !== undefined && !next.promptSummaries.has(summary.input_id)) {
      next = { ...next, promptSummaries: new Map(next.promptSummaries).set(summary.input_id, summary) };
    }
  }
  for (const id of replayed) next = replay(next, id).view;
  return next;
}

/** Offers the pieces that waited for an item to the item, now that a row for it arrived. */
function replay(view: ThreadView, itemId: string): { view: ThreadView; gap: boolean } {
  const waiting = view.held.get(itemId);
  if (waiting === undefined) return { view, gap: false };
  const item = view.items.get(itemId);
  const projection = view.read?.projection;
  if (projection !== undefined && projection !== null && item !== undefined
    && item.continuation_id !== projection.viewed_continuation_id) {
    const held = new Map(view.held); held.delete(itemId);
    const gaps = new Set(view.gaps); gaps.delete(itemId);
    return { view: { ...view, held, gaps }, gap: false };
  }
  let next: ThreadView = view;
  let gap = false;
  const remaining: Delta[] = [];
  for (const piece of [...waiting].sort((one, other) => one.offset - other.offset)) {
    const item = next.items.get(itemId);
    if (item === undefined) {
      remaining.push(piece);
      continue;
    }
    const outcome = applyDelta(item, next.applied.get(itemId)?.[piece.field] ?? 0, piece);
    if (outcome.kind === 'applied') next = writeProgress(next, itemId, piece, outcome.item, outcome.applied);
    else if (outcome.kind === 'gap') {
      remaining.push(piece);
      gap = true;
    }
  }
  const held = new Map(next.held);
  if (remaining.length > 0) held.set(itemId, remaining);
  else held.delete(itemId);
  const gaps = new Set(next.gaps);
  if (!gap) gaps.delete(itemId);
  return { view: { ...next, held, gaps }, gap };
}

function writeProgress(view: ThreadView, itemId: string, piece: Delta, item: Item, applied: number): ThreadView {
  const previous = view.applied.get(itemId)?.text ?? 0;
  const appended = piece.field === 'text' ? dropCodePoints(piece.data, Math.max(0, previous - piece.offset)) : undefined;
  const outputCounts = observeOutput(view.outputCounts, view.items.get(itemId), item, appended);
  const items = new Map(view.items);
  items.set(itemId, item);
  const all = new Map(view.applied);
  all.set(itemId, { ...(all.get(itemId) ?? {}), [piece.field]: applied });
  return { ...view, items, applied: all, outputCounts };
}

/** What applying a batch of pieces came to. */
export interface DeltaResult {
  view: ThreadView;
  /** Items that must be read again: a piece began beyond what was held of them. */
  gaps: string[];
}

/** Adds pieces of growing text to their items. A piece whose item is not held, or is being read again, waits. */
export function applyDeltas(view: ThreadView, pieces: readonly Delta[]): DeltaResult {
  let next = view;
  const gaps: string[] = [];
  const hold = (piece: Delta) => {
    const held = new Map(next.held);
    const list = mergeDeltas([...(held.get(piece.item_id) ?? []), piece]);
    held.set(piece.item_id, list.length > HELD_PIECES_MOST ? list.slice(list.length - HELD_PIECES_MOST) : list);
    next = { ...next, held };
  };
  // Pieces that follow one another are one change; the work of a frame is the work of the last piece of each item.
  const grouped = new Map<string, Delta[]>();
  for (const piece of pieces) grouped.set(piece.item_id, [...(grouped.get(piece.item_id) ?? []), piece]);
  for (const list of grouped.values()) {
    for (const piece of mergeDeltas(list)) {
      const item = next.items.get(piece.item_id);
      // Prefix bodies belong to an immutable observation even if their shared pin/quote row changes.
      if (next.read?.projection !== null && next.read?.projection !== undefined && item !== undefined
        && item.continuation_id !== next.read.projection.viewed_continuation_id) continue;
      if (item === undefined || next.gaps.has(piece.item_id)) {
        hold(piece);
        continue;
      }
      const outcome = applyDelta(item, next.applied.get(piece.item_id)?.[piece.field] ?? 0, piece);
      if (outcome.kind === 'applied') next = writeProgress(next, piece.item_id, piece, outcome.item, outcome.applied);
      else if (outcome.kind === 'gap') {
        hold(piece);
        next = { ...next, gaps: new Set(next.gaps).add(piece.item_id) };
        gaps.push(piece.item_id);
      }
    }
  }
  return { view: next, gaps };
}

/** A whole answer replaces what was held, as a load from nothing does. */
export function viewFromRows(previous: ThreadView, rows: ThreadRows, hasOlder: boolean): ThreadView {
  const fresh = applyRows(
    {
      ...EMPTY_VIEW,
      ref: previous.ref,
      read: previous.read,
      promptSummaries: previous.promptSummaries,
      turns: previous.turns,
      segments: previous.segments,
      loaded: 'tail',
      hasOlder,
      presence: previous.presence,
      // Where the person had read stays where it was first seen, however often the thread is loaded again.
      openedReadSeq: previous.openedReadSeq ?? rows.thread?.my_last_read_seq ?? null,
      openedSeq: previous.openedSeq ?? rows.thread?.seq ?? null,
    },
    rows,
  );
  // A full reload replaces content, while this same reader's newer icon acknowledgement still holds.
  const display = previous.closed === null && fresh.thread?.id === previous.thread?.id
    ? currentThreadDisplay(fresh.thread?.display, previous.thread?.display) : fresh.thread?.display;
  return prunePromptMetadata(fresh.thread === null || display === undefined || display === fresh.thread.display
    ? fresh : { ...fresh, thread: { ...fresh.thread, display } });
}

// ---------------------------------------------------------------- the store

interface ThreadStore {
  /** Legacy name: default native IDs and independent local window keys share this one map. */
  byThread: ReadonlyMap<string, ThreadView>;
}

export const useThreadStore = create<ThreadStore>(() => ({ byThread: new Map() }));

function update(threadId: ThreadReadTarget, change: (view: ThreadView) => ThreadView): void {
  const ref = threadReadTarget(threadId);
  const key = threadReadCacheKey(ref);
  useThreadStore.setState((state) => {
    const before = state.byThread.get(key) ?? EMPTY_VIEW;
    const after = change(before);
    if (after === before) return state;
    const byThread = new Map(state.byThread);
    byThread.set(key, { ...after,ref });
    return { byThread };
  });
}

/** Selection is owned by the existing cache entry and evicted with it. No second branch map. */
let onReadChange: (threadId: string, ref: ThreadReadRef) => void = () => undefined;
let retainRead: (threadId: string, ref: ThreadReadRef) => void = () => undefined;
let holdRead: (threadId: string, ref: ThreadReadRef) => () => void = () => () => undefined;
export function setThreadReadHolder(handler: (threadId: string, ref: ThreadReadRef) => () => void): void { holdRead = handler; }
export function holdThreadRead(threadId: ThreadReadTarget): () => void {
  const ref = threadReadTarget(threadId);
  return holdRead(ref.threadId,ref);
}
/** The application joins even an HTTP-only read to the existing feed-cache eviction lifetime. */
export function setThreadReadRetainer(handler: (threadId: string, ref: ThreadReadRef) => void): void { retainRead = handler; }
export function setThreadReadChangeHandler(handler: (threadId: string, ref: ThreadReadRef) => void): void { onReadChange = handler; }

function withoutHistory(view: ThreadView, read: ThreadReadScope): ThreadView {
  return { ...EMPTY_VIEW, thread: view.thread, presence: view.presence, read };
}

/** A requested locator is not authority: retain it while replacing every reader/content ticket. */
function rebasedRead(view: ThreadView, ref: ThreadReadRef, reader: number | undefined): ThreadReadScope {
  const fresh = newThreadReadScope(reader,ref), before = view.read;
  return before === null ? fresh : { ...fresh,selection: before.selection,provisional: before.provisional,
    switched: before.switched || before.projection !== null };
}

/** A returned row keeps its receipt when a current control puts it in a fresh array. */
function accepts(view: ThreadView, threadId: ThreadReadTarget, rows: ThreadRows, captured?: ThreadReadCapture): boolean {
  const scope = view.read;
  if (view.closed !== null || rows.thread !== undefined && rows.thread !== null
    && rows.thread.id !== threadReadTarget(threadId).threadId) return false;
  const projected = scope?.projection;
  if (projected !== undefined && projected !== null && (rows.inputs ?? []).some((row) => row.state !== 'delivered'
    && row.continuation_id !== undefined && row.continuation_id !== null
    && row.continuation_id !== projected.viewed_continuation_id)) return false;
  if (captured !== undefined) return sameThreadRead(captured.ref,threadId) && matchesThreadCapture(scope, captured)
    && scope !== null && matchesThreadProjection(scope, captured);
  const objects: object[] = [...(rows.items ?? []), ...(rows.turns ?? []), ...(rows.inputs ?? []), ...(rows.segments ?? [])];
  if (rows.thread !== undefined && rows.thread !== null) objects.push(rows.thread);
  for (const row of objects) {
    const receipt = threadReceipt(row);
    if (receipt !== undefined && (!sameThreadRead(receipt.ref,threadId) || !matchesThreadCapture(scope, receipt)
      || scope === null || !matchesThreadProjection(scope, receipt))) return false;
    if (receipt === undefined && scope?.switched === true && row !== rows.thread) return false;
  }
  return true;
}

export function threadReadOnly(view: ThreadView): boolean {
  return view.closed !== null || view.read !== null && (view.read.selection !== null || !view.read.ready
    || view.read.projection !== null && view.read.projection.viewed_continuation_id !== view.read.projection.active_continuation_id);
}

/** Read readiness is not native preparation status; viewedPrompt supplies that independent fact. */
export function threadReadState(view: ThreadView): {
  selection: string | null;
  projection: ThreadContinuationProjection | null;
  readiness: 'loading' | 'ready' | 'failed' | 'closed';
  readOnly: boolean;
} {
  const readiness = view.closed !== null ? 'closed' : view.error !== null ? 'failed'
    : view.loaded === 'none' || view.read?.ready === false ? 'loading' : 'ready';
  return { selection: view.read?.selection ?? null, projection: view.read?.projection ?? null,
    readiness, readOnly: readiness !== 'ready' || threadReadOnly(view) };
}

/** Asked to read an item again, when a piece of its growing text began beyond what was held. */
let onGap: (threadId: string, itemId: string, captured?: ThreadReadCapture) => void = () => undefined;

/** How long a growing item that was read back still behind its waiting pieces rests before it is read again: about
 *  half of the second the server holds growing text before writing it (`[engine] stream_flush_ms`). */
export const GAP_READ_AGAIN_MS = 500;

/** Says who reads an item again when a gap shows. The live connection sets it. */
export function setGapHandler(handler: (threadId: string, itemId: string, captured?: ThreadReadCapture) => void): void {
  onGap = handler;
}

const queued = new Map<string, { ref: ThreadReadRef; pieces: Delta[]; captured?: ThreadReadCapture }>();
let frameHandle: number | null = null;
let timerHandle: ReturnType<typeof setTimeout> | null = null;

/** The pieces of every thread that wait for the next frame are added to their items. */
export function flushDeltas(): void {
  if (frameHandle !== null) cancelAnimationFrame(frameHandle);
  if (timerHandle !== null) clearTimeout(timerHandle);
  frameHandle = null;
  timerHandle = null;
  for (const [key, batch] of queued) {
    queued.delete(key);
    const gaps: string[] = [];
    update(batch.ref, (view) => {
      if (batch.captured !== undefined && (!matchesThreadCapture(view.read, batch.captured)
        || view.read === null || !matchesThreadProjection(view.read, batch.captured))
        || batch.captured === undefined && view.read?.switched === true || view.closed !== null) return view;
      const result = applyDeltas(view, batch.pieces);
      gaps.push(...result.gaps);
      return result.view;
    });
    for (const itemId of gaps) onGap(batch.ref.threadId, itemId, batch.captured);
  }
}

/** The pace of a frame where the page has none to give (a hidden tab draws none; a test has none). */
const FRAME_FALLBACK_MS = 16;

function schedule(): void {
  if (frameHandle !== null || timerHandle !== null) return;
  if (typeof requestAnimationFrame === 'function' && typeof document !== 'undefined' && document.visibilityState === 'visible') {
    frameHandle = requestAnimationFrame(flushDeltas);
  } else {
    timerHandle = setTimeout(flushDeltas, FRAME_FALLBACK_MS);
  }
}

/** The writers. Only the live connection and the person's own acts call them. */
export const threadWriter = {
  /** Captures the one cache lifetime; a new reader never inherits its predecessor's rows. */
  capture(threadId: ThreadReadTarget, reader: number | undefined): ThreadReadCapture {
    const ref = threadReadTarget(threadId);
    update(threadId, (view) => {
      const read = view.read;
      if (read !== null && read.reader === reader && sameThreadRead(read.ref,ref)) return view;
      read?.controller.abort();
      const fresh = rebasedRead(view,ref,reader);
      return read === null ? { ...view, read: fresh } : { ...EMPTY_VIEW, read: fresh };
    });
    const read = threadViewOf(threadId).read;
    if (read === null) throw new Error('The thread read lifetime was not captured');
    retainRead(ref.threadId,ref);
    return { ref,threadId: ref.threadId, scope: read };
  },

  /** Invalidates outstanding work once; an already-loading read keeps the same demand. */
  beginRead(threadId: ThreadReadTarget, reader: number | undefined): ThreadReadCapture {
    this.capture(threadId, reader);
    queued.delete(threadReadCacheKey(threadId));
    update(threadId, (view) => {
      if (view.read === null) return view;
      const retire = view.read.ready || view.read.provisional || view.read.controller.signal.aborted;
      if (retire) view.read.controller.abort();
      return { ...view, closed: null, read: { ...view.read, ready: false, provisional: false, opening: true,
        epoch: view.read.epoch + (retire ? 1 : 0), controller: retire ? new AbortController() : view.read.controller } };
    });
    return this.capture(threadId, reader);
  },

  /** Selection only reads; this cannot submit, resume or activate work. */
  select(threadId: ThreadReadTarget, selection: string | null, reader: number | undefined): void {
    this.capture(threadId, reader);
    const before = threadViewOf(threadId).read;
    if (before === null || before.selection === selection) return;
    before.controller.abort();
    update(threadId, (view) => view.read === null ? view : withoutHistory(view,
      { ...view.read, selection, projection: null, epoch: view.read.epoch + 1, switched: true, ready: false,
        provisional: false, opening: false, controller: new AbortController() }));
    queued.delete(threadReadCacheKey(threadId));
    onReadChange(threadReadTarget(threadId).threadId,threadReadTarget(threadId));
  },

  /** A known gap/scope reset retires outstanding receipts before the new bounded opening. */
  invalidate(threadId: ThreadReadTarget, notify = true): void {
    queued.delete(threadReadCacheKey(threadId));
    threadViewOf(threadId).read?.controller.abort();
    update(threadId, (view) => view.read === null ? view : withoutHistory(view,
      { ...view.read, epoch: view.read.epoch + 1, projection: null, ready: false, switched: true,
        provisional: false, opening: false, controller: new AbortController() }));
    if (notify) onReadChange(threadReadTarget(threadId).threadId,threadReadTarget(threadId));
  },

  /** Takes a whole answer; historical references survive only within the same projection. */
  reset(threadId: ThreadReadTarget, rows: ThreadRows, hasOlder: boolean, captured?: ThreadReadCapture, planned?: ThreadReadScope): ThreadReadCapture | null {
    let accepted: ThreadReadCapture | null = null;
    update(threadId, (view) => {
      if (rows.thread !== undefined && rows.thread !== null && rows.thread.id !== threadReadTarget(threadId).threadId) return view;
      if (captured !== undefined && (!sameThreadRead(captured.ref,threadId) || !matchesThreadCapture(view.read, captured))) return view;
      if (captured === undefined && !accepts(view, threadId, rows)) return view;
      let read = view.read;
      let base = view;
      const projection = rows.projection ?? null;
      if (read !== null) {
        const next = planned ?? planThreadOpening(read, projection);
        if (next?.identity !== read.identity || next.reader !== read.reader
          || next.selection !== read.selection || next.epoch < read.epoch) return view;
        const changed = next.epoch !== read.epoch;
        if (changed) read.controller.abort();
        read = next;
        if (changed) base = withoutHistory(view, read);
        accepted = { ref: read.ref,threadId: read.ref.threadId, scope: read };
      }
      return viewFromRows({ ...base, read }, rows, hasOlder);
    });
    return accepted;
  },

  /** Rows without their captured scope cannot enter a selected modern history. */
  apply(threadId: ThreadReadTarget, rows: ThreadRows, captured?: ThreadReadCapture): void {
    update(threadId, (view) => {
      if (!accepts(view, threadId, rows, captured)) return view;
      // Global title/settings acts carry the active person's ordinary Thread view. Retain this
      // history's independent read marker unless the row came through its own admitted cut.
      let admitted = rows;
      if (captured === undefined && rows.thread !== undefined && rows.thread !== null
        && rows.projection === undefined && threadReceipt(rows.thread) === undefined
        && view.read?.projection !== undefined && view.read.projection !== null) {
        admitted = { ...rows, thread: { ...rows.thread, my_last_read_seq: view.thread?.my_last_read_seq ?? null } };
      }
      const next = applyRows(view, admitted);
      return next.error === null || next.read?.ready === false ? next : { ...next, error: null };
    });
  },

  queueLoading(threadId: ThreadReadTarget, captured: ThreadReadCapture, request: object): void {
    update(threadId, (view) => matchesThreadCapture(view.read, captured)
      ? { ...view, queuePage: { ...view.queuePage, loading: true, error: null, request } } : view);
  },

  queueLoaded(threadId: ThreadReadTarget, page: ThreadInputPage, captured: ThreadReadCapture, first: boolean, request: object): void {
    update(threadId, (view) => {
      if (view.queuePage.request !== request || !matchesThreadCapture(view.read, captured)
        || view.read === null || !matchesThreadProjection(view.read, captured)
        || view.read.projection === null || !sameThreadProjection(view.read.projection, page.projection)
        || !first && page.queue_revision !== view.queuePage.revision) return view;
      const referenced = referencedInputs(view);
      const inputs = first ? new Map([...view.inputs].filter(([id]) => referenced.has(id))) : new Map(view.inputs);
      const tail = first ? new Set<string>() : new Set(view.queuePage.tail);
      for (const row of page.inputs) { inputs.set(row.id, stampThreadReceipt(row, captured)); if (!first) tail.add(row.id); }
      return prunePromptMetadata({ ...view, inputs, queuePage: { nextAfter: page.next_after,
        headNextAfter: first ? page.next_after : view.queuePage.headNextAfter, revision: page.queue_revision,
        loading: false, error: null, restartRequired: false, tail, request: null } });
    });
  },

  queueFailed(threadId: ThreadReadTarget, error: unknown, captured: ThreadReadCapture, request: object): void {
    update(threadId, (view) => matchesThreadCapture(view.read, captured) && view.queuePage.request === request
      ? { ...view, queuePage: { ...view.queuePage, loading: false, error, restartRequired: true, request: null } } : view);
  },

  queueCancelled(threadId: ThreadReadTarget, captured: ThreadReadCapture, request: object): void {
    update(threadId, (view) => matchesThreadCapture(view.read, captured) && view.queuePage.request === request
      ? { ...view, queuePage: { ...view.queuePage, loading: false, request: null } } : view);
  },

  /**
   * A change to the thread row that the person made themselves and sees at once (a bell, a star): it is
   * put over the row held, whatever its sequence, and the next row from the server replaces it.
   */
  patch(threadId: ThreadReadTarget, change: (thread: Thread) => Thread): void {
    update(threadId, (view) => (view.thread === null ? view : { ...view, thread: change(view.thread) }));
  },

  /** A piece of growing text; it is added with the next frame. */
  delta(threadId: ThreadReadTarget, piece: Delta, captured?: ThreadReadCapture): void {
    const view = threadViewOf(threadId);
    // An owner is a locator. It cannot manufacture a current ticket for a delayed old delta.
    const owned = captured;
    if (owned !== undefined && (!matchesThreadCapture(view.read, owned)
      || view.read === null || !matchesThreadProjection(view.read, owned))
      || owned === undefined && view.read?.switched === true || view.closed !== null) return;
    const key = threadReadCacheKey(threadId);
    const batch = queued.get(key);
    queued.set(key, { ref: threadReadTarget(threadId), pieces: [...(batch?.pieces ?? []), piece], ...(owned === undefined ? {} : { captured: owned }) });
    schedule();
  },

  /** An item that was read again after a gap: it says how far the item has come, and the pieces that waited go on from there. */
  itemFetched(threadId: ThreadReadTarget, item: Item, captured?: ThreadReadCapture): void {
    const behind: string[] = [];
    update(threadId, (view) => {
      if (!accepts(view, threadId, { items: [item] }, captured)) return view;
      let next: ThreadView = view;
      const held = view.items.get(item.id);
      if (held === undefined || held.seq_updated <= item.seq_updated) {
        const items = new Map(view.items);
        const applied = new Map(view.applied);
        items.set(item.id, item);
        applied.set(item.id, appliedOf(item));
        next = { ...view, items, applied };
        if (held === undefined && item.stream === MAIN_STREAM && !(view.loaded === 'window' && view.hasNewer)) {
          next = { ...next, order: mergeOrder(next.order, [item.id], items) };
        }
      }
      const replayed = replay(next, item.id);
      if (replayed.gap) behind.push(item.id);
      return replayed.view;
    });
    // The server writes growing text a moment after it sends its pieces, so an item read in that moment can still
    // end before the pieces that wait. While it grows, it is read again shortly; the pieces go on from that answer.
    if (behind.length > 0 && item.status === 'in_progress') {
      setTimeout(() => {
        if (threadViewOf(threadId).gaps.has(item.id)) onGap(threadReadTarget(threadId).threadId, item.id, captured);
      }, GAP_READ_AGAIN_MS);
    }
  },

  /** A read of an item that was begun after a gap did not succeed: a later piece may begin it again. */
  gapFailed(threadId: ThreadReadTarget, itemId: string, captured?: ThreadReadCapture): void {
    update(threadId, (view) => {
      if (captured !== undefined && !matchesThreadCapture(view.read, captured)) return view;
      if (!view.gaps.has(itemId)) return view;
      const gaps = new Set(view.gaps);
      gaps.delete(itemId);
      return { ...view, gaps };
    });
  },

  /** Older items, read when the reader came to the top. */
  older(threadId: ThreadReadTarget, rows: readonly Item[], hasOlder: boolean, captured?: ThreadReadCapture): void {
    update(threadId, (view) => accepts(view, threadId, { items: rows }, captured)
      ? prunePromptMetadata({ ...applyItems(view, rows), hasOlder }) : view);
  },

  /** A stretch around an item, read for a link to it: it takes the place of what was held, and is not the end. */
  window(threadId: ThreadReadTarget, rows: readonly Item[], hasOlder: boolean, hasNewer: boolean, captured?: ThreadReadCapture): void {
    update(threadId, (view) => {
      if (!accepts(view, threadId, { items: rows }, captured)) return view;
      const base: ThreadView = {
        ...view,
        loaded: hasNewer ? 'window' : 'tail',
        hasOlder,
        hasNewer,
        items: new Map(),
        order: [],
        streams: new Map(),
        applied: new Map(),
        held: new Map(),
        gaps: new Set(),
      };
      // A window shows its own items, whatever the stream says about being at the end.
      const filled = applyItems({ ...base, hasNewer: false, loaded: 'tail' }, rows);
      return prunePromptMetadata({ ...filled, loaded: base.loaded, hasNewer, hasOlder });
    });
  },

  presence(threadId: ThreadReadTarget, users: readonly PresenceUser[]): void {
    update(threadId, (view) => ({ ...view, presence: users }));
  },

  /** Records that the thread could not be loaded. What is held stays, to be read greyed. */
  failed(threadId: ThreadReadTarget, error: unknown): void {
    update(threadId, (view) => ({ ...view, error, read: view.read === null ? null : { ...view.read, opening: false } }));
  },

  /** Losing the required protocol identity retires controls/tickets, without discarding greyed rows. */
  refused(threadId: ThreadReadTarget, error: unknown, captured: ThreadReadCapture): void {
    const held = threadViewOf(threadId).read;
    if (!matchesThreadCapture(held, captured) || held === null || !matchesThreadProjection(held, captured)) return;
    queued.delete(threadReadCacheKey(threadId));
    held.controller.abort();
    update(threadId, (view) => {
      if (view.read !== held) return view;
      return { ...view, error, read: { ...held, epoch: held.epoch + 1, ready: false,
        provisional: false, opening: false, controller: new AbortController() } };
    });
  },

  /** Updates known local windows from the server's current-execution icon, without opening a feed. */
  display(threadId: string, display: ThreadDisplay): void {
    for (const view of useThreadStore.getState().byThread.values()) {
      if (view.closed !== null || view.thread?.id !== threadId || currentThreadDisplay(display, view.thread.display) !== display) continue;
      update(view.ref ?? threadId, (held) => held.thread === null ? held : { ...held, thread: { ...held.thread, display } });
    }
  },

  /** Personal read state applies only to this viewed continuation; it never selects execution. */
  readState(threadId: ThreadReadTarget, continuation: string | null, seq: number): void {
    update(threadId, (view) => {
      const projection = view.read?.projection;
      const legacy = continuation === null;
      if (legacy && (view.read?.selection !== undefined && view.read.selection !== null
        || projection !== undefined && projection !== null)
        || !legacy && projection?.viewed_continuation_id !== continuation) return view;
      if (view.thread === null || (view.thread.my_last_read_seq ?? 0) >= seq) return view;
      return { ...view, thread: { ...view.thread, my_last_read_seq: seq } };
    });
  },

  /** A person's native marker belongs to the actual thread/continuation across known local windows. */
  readStateAll(threadId: string, continuation: string | null, seq: number): void {
    for (const view of useThreadStore.getState().byThread.values()) {
      const ref = view.ref;
      if (ref !== null && ref.threadId === threadId) this.readState(ref,continuation,seq);
    }
  },

  /** The server ended the feed. */
  closed(threadId: ThreadReadTarget, reason: 'revoked' | 'deleted'): void {
    queued.delete(threadReadCacheKey(threadId));
    threadViewOf(threadId).read?.controller.abort();
    update(threadId, (view) => ({ ...view, closed: reason,
      read: view.read === null ? null : { ...view.read, epoch: view.read.epoch + 1, ready: false } }));
  },

  /** Authority changes remove data/tickets, never silently turn explicit B into active A. */
  rebase(threadId: ThreadReadTarget, reader: number | undefined): void {
    queued.delete(threadReadCacheKey(threadId));
    threadViewOf(threadId).read?.controller.abort();
    update(threadId,(view) => ({ ...EMPTY_VIEW,read: rebasedRead(view,threadReadTarget(threadId),reader) }));
  },

  /** A new credential cannot retain any old body, even in a view without a live registry follower. */
  rebaseAll(reader: number | undefined): void {
    queued.clear();
    useThreadStore.setState((state) => {
      const byThread = new Map<string,ThreadView>();
      for (const [key,view] of state.byThread) {
        view.read?.controller.abort();
        const ref = view.ref ?? view.read?.ref;
        if (ref !== undefined) byThread.set(key,{ ...EMPTY_VIEW,ref,read: rebasedRead(view,ref,reader) });
      }
      return { byThread };
    });
  },

  /** Forgets a thread nobody shows any more. */
  evict(threadId: ThreadReadTarget): void {
    threadViewOf(threadId).read?.controller.abort();
    queued.delete(threadReadCacheKey(threadId));
    useThreadStore.setState((state) => {
      const key = threadReadCacheKey(threadId);
      if (!state.byThread.has(key)) return state;
      const byThread = new Map(state.byThread);
      byThread.delete(key);
      return { byThread };
    });
  },

  /** Forgets everything: whoever was signed in has left. */
  clear(): void {
    for (const view of useThreadStore.getState().byThread.values()) view.read?.controller.abort();
    queued.clear();
    useThreadStore.setState({ byThread: new Map() });
  },
};

// ---------------------------------------------------------------- reading

/** A thread as far as it has arrived. */
export function useThreadView(threadId: ThreadReadTarget): ThreadView {
  const ref = useThreadReadRef(threadId);
  const key = threadReadCacheKey(ref);
  return useStore(useThreadStore, (state) => state.byThread.get(key) ?? EMPTY_VIEW);
}

/** One piece of a thread, by a selector; the component draws again only when that piece changes. */
export function useThreadPart<Part>(threadId: ThreadReadTarget, select: (view: ThreadView) => Part): Part {
  const ref = useThreadReadRef(threadId);
  const key = threadReadCacheKey(ref);
  return useStore(useThreadStore, (state) => select(state.byThread.get(key) ?? EMPTY_VIEW));
}

/** The thread row itself. */
export function useThreadRow(threadId: ThreadReadTarget): Thread | null {
  return useThreadPart(threadId, (view) => view.thread);
}

/** An item by its id; it is drawn again only when this item changes. */
export function useItem(threadId: ThreadReadTarget, itemId: string): Item | undefined {
  return useThreadPart(threadId, (view) => view.items.get(itemId));
}

/** An input by its id. */
export function useInput(threadId: ThreadReadTarget, inputId: string): ThreadInput | undefined {
  return useThreadPart(threadId, (view) => view.inputs.get(inputId));
}

/** The view of a thread outside a component. */
export function threadViewOf(threadId: ThreadReadTarget): ThreadView {
  return useThreadStore.getState().byThread.get(threadReadCacheKey(threadId)) ?? EMPTY_VIEW;
}

// ---------------------------------------------------------------- what the screens derive

/** The latest change of a thread's turns, including a context breakdown that arrives after the turn. */
export function turnsRevision(view: ThreadView): number {
  let latest = 0;
  for (const turn of view.turns.values()) latest = Math.max(latest, turn.seq_updated);
  return latest;
}

/** The turn under way, if the thread has one. */
export function runningTurn(view: ThreadView): Turn | null {
  const id = view.thread?.current_turn_id ?? null;
  const turn = id === null ? undefined : view.turns.get(id);
  // History can retain a running row while its terminal update is in flight. Only the
  // authoritative current-turn pointer names today's work; an older row cannot revive it.
  return turn !== undefined && (turn.state === 'running' || turn.state === 'starting') ? turn : null;
}

/** The inputs that wait or are being handed over, in delivery order. */
export function waitingInputs(view: ThreadView): ThreadInput[] {
  return [...view.inputs.values()]
    .filter((input) => input.state === 'pending' || input.state === 'delivering')
    .sort((one, other) => comparePositions(one.position, other.position));
}
