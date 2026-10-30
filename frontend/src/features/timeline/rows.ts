/** The rows the conversation is drawn as: which items stand alone, which are folded into one line for their turn, and where the lines that belong to no item go.
 *
 * The items of a thread are in the order they were made. What is drawn is decided here, apart from
 * any screen, so that it can be tested by what a reader would see: the work of a turn is shown step
 * by step or folded to one line by the depth the reader chose (Steps, Summary or Everything); lines
 * of the thread's history that one person made within a minute become one; the "New" line stands
 * before the first thing the reader has not seen; a turn that is over ends in its footer; the next
 * waiting inputs belong to the queue above the composer; and what the thread is doing now, when it
 * is no item, is the last line.
 */
import { approvalSnapshot } from '@/features/approvals/snapshot';
import type { SendingInput } from '@/features/composer/Draft';
import type { Item, Thread, ThreadInput, Turn } from '@/generated/api-types';
import { runningTurn, threadReadOnly, type ThreadView, waitingInputs } from '@/realtime/stores/thread';
import { hiddenFailureItem } from './failures';

export type WorkDetail = 'steps' | 'summary' | 'everything';

/** Lines of the thread's history by one person this close together become one line with a count. */
export const MARKERS_JOINED_WITHIN_MS = 60_000;

export type Row =
  /** An item drawn on its own: a message, a step, a plan, an approval, an error. `head`: the first thing the agent says in its turn, which the agent's mark and name go before. */
  | { kind: 'item'; key: string; id: string; head?: true }
  /** The work of a turn, folded to one line; `open` shows its steps under the line. */
  | { kind: 'fold'; key: string; turnId: string; ids: readonly string[]; open: boolean }
  /** Lines of the thread's history, joined: the first is drawn with how many there were. */
  | { kind: 'markers'; key: string; ids: readonly string[] }
  /** An aside at the turn it was asked from; its closing marker settles the same row. */
  | { kind: 'side'; key: string; id: string; closedId?: string; sourceTurn?: number }
  /** The end of a turn that is over. */
  | { kind: 'footer'; key: string; turnId: string; inputItemId?: string }
  /** Where what the reader had not seen begins. */
  | { kind: 'divider'; key: string }
  /** What the thread is doing, when that is no item. */
  | { kind: 'status'; key: string }
  /** Where what an adopted session held before Orch took it in begins: the heading of those items. */
  | { kind: 'before'; key: string };

export interface RowOptions {
  detail: WorkDetail;
  hideHistory: boolean;
  /** Turns whose folded work the reader opened by hand: its steps stand under its line. */
  foldOpened: ReadonlySet<string>;
  /** The person looking: what they wrote themselves is never "new" to them. */
  meId: string | null;
}

/** Items that are the agent's work between what it says: they fold when their turn does. */
export function isWork(item: Item): boolean {
  return item.kind === 'tool_call' || item.kind === 'reasoning' || item.kind === 'plan';
}

/** The turns whose work is shown step by step in Steps: the one that runs and the last one that is over. */
export function recentTurns(turns: ReadonlyMap<string, Turn>): ReadonlySet<string> {
  const recent = new Set<string>();
  let lastOver: Turn | null = null;
  for (const turn of turns.values()) {
    if (turn.state === 'running' || turn.state === 'starting') recent.add(turn.id);
    else if (lastOver === null || turn.ordinal > lastOver.ordinal) lastOver = turn;
  }
  if (lastOver !== null) recent.add(lastOver.id);
  return recent;
}

/** Whether the work of a turn is folded, before the reader's own opening and closing. */
export function foldsByDepth(detail: WorkDetail, turnId: string, recent: ReadonlySet<string>): boolean {
  if (detail === 'everything') return false;
  if (detail === 'summary') return true;
  return !recent.has(turnId);
}

/** Whether a turn is over. */
export function isOver(turn: Turn): boolean {
  return turn.state === 'completed' || turn.state === 'interrupted' || turn.state === 'failed';
}

function markerAuthor(item: Item): string | null {
  if (item.kind !== 'marker') return null;
  const data = item.payload.data as { by?: { id?: string } | null };
  return data.by?.id ?? null;
}

/** What the thread is doing that is no item: it ends the conversation as a line. */
export function hasStatusLine(thread: Thread | null, view: ThreadView): boolean {
  if (thread === null || threadReadOnly(view)) return false;
  if (thread.state === 'blocked' && thread.blocked?.detail === 'call_recovery') return true;
  // Other blocked reasons belong to the banner above the box.
  if (thread.lock !== null || thread.state === 'starting' || thread.state === 'waiting_approval') return true;
  return runningTurn(view) !== null;
}

export function buildRows(view: ThreadView, options: RowOptions): Row[] {
  const { items, turns } = view;
  const contentSources = new Set<string>();
  for (const item of items.values()) {
    if (item.kind !== 'approval' || item.parent_item_id === null) continue;
    const approval = approvalSnapshot(item.payload.approval, item.payload.approval_id);
    if (approval?.kind !== 'harness' || !['user_input', 'mcp_elicitation'].includes(approval.request.request_kind)) continue;
    const source = items.get(item.parent_item_id);
    if (source?.kind === 'tool_call' && source.thread_id === item.thread_id && source.turn_id === item.turn_id) contentSources.add(source.id);
  }
  // A content card owns its native work detail. Keep the item in the store, not a second question row.
  const order = view.order.filter((id) => !contentSources.has(id));
  const rows: Row[] = [];
  const recent = recentTurns(turns);

  // The turns whose work has a step in it, and the last place each turn is seen in the order.
  const lastOfTurn = new Map<string, {last:number;inputItemId?:string}>();
  const steps = new Map<string, string[]>();
  const contextTurns = new Set<string>();
  const toolTurns=new Set<string>();
  order.forEach((id, index) => {
    const item = items.get(id);
    const turnId = item?.turn_id ?? null;
    if (item === undefined || turnId === null) return;
    let entry=lastOfTurn.get(turnId);
    if(entry===undefined){entry={last:index};lastOfTurn.set(turnId,entry);}else entry.last=index;
    if(item.kind==='user_message'&&entry.inputItemId===undefined)entry.inputItemId=item.id;
    if (item.kind === 'marker' && item.payload.marker === 'orch_context') contextTurns.add(turnId);
    if(item.kind==='tool_call')toolTurns.add(turnId);
    if(isWork(item)){const group=steps.get(turnId);if(group===undefined)steps.set(turnId,[id]);else group.push(id);}
  });
  const hasToolCall = (turnId: string): boolean => toolTurns.has(turnId);
  const folded = (turnId: string): boolean => hasToolCall(turnId) && foldsByDepth(options.detail, turnId, recent);

  // Start markers arrive at the end of the main stream, but carry their actual history boundary.
  // Relocate only to a held turn: a partial history window must not fetch unrelated history or lose
  // an aside whose source is outside it. Such a row names its source until that turn is read.
  const starts = new Map<string, Item>();
  const closed = new Map<string, string>();
  for (const id of order) {
    const item = items.get(id);
    if (item?.kind !== 'marker') continue;
    if (item.payload.marker === 'side_started') starts.set(item.payload.data.side.id, item);
    else if (item.payload.marker === 'side_closed') closed.set(item.payload.data.side.id, id);
  }
  const sideAfter = new Map<number, Row[]>();
  const sideBefore = new Map<number, Row[]>();
  const positions = new Map(order.map((id, index) => [id, index]));
  const endsByOrdinal = new Map([...turns.values()].map((turn) => [turn.ordinal, lastOfTurn.get(turn.id)?.last]));
  const firstTurnIndex = view.hasOlder ? -1 : order.findIndex((id) => { const item = items.get(id); return item !== undefined && item.turn_id !== null; });
  for (const item of starts.values()) {
    if (item.kind !== 'marker' || item.payload.marker !== 'side_started') continue;
    const { from_turn: ordinal, side } = item.payload.data;
    const markerIndex = positions.get(item.id);
    if (markerIndex === undefined) continue;
    const last = endsByOrdinal.get(ordinal);
    const first = ordinal === 0 ? firstTurnIndex : -1;
    const anchored = last !== undefined || first >= 0;
    const index = last ?? (first >= 0 ? first : markerIndex);
    const closedId = closed.get(side.id);
    const row: Row = { kind: 'side', key: item.id, id: item.id, ...(closedId === undefined ? {} : { closedId }),
      ...(anchored ? {} : { sourceTurn: ordinal }) };
    const target = first >= 0 ? sideBefore : sideAfter;
    target.set(index, [...(target.get(index) ?? []), row]);
  }

  // What the reader had not seen when they opened the thread; what arrived since they watched arrive.
  const dividerAfter = view.openedReadSeq !== null && view.openedSeq !== null && view.openedReadSeq < view.openedSeq ? view.openedReadSeq : null;
  const dividerUntil = view.openedSeq ?? 0;
  let dividerPlaced = dividerAfter === null;
  const placeDivider = (item: Item) => {
    if (dividerPlaced || dividerAfter === null) return;
    if (item.seq_created > dividerAfter && item.seq_created <= dividerUntil && item.author?.id !== options.meId) {
      rows.push({ kind: 'divider', key: 'divider' });
      dividerPlaced = true;
    }
  };

  const foldPlaced = new Set<string>();
  const spoken = new Set<string>();
  // What a session held before it was adopted stands under one heading, where the first of it stands.
  let beforePlaced = false;
  let pendingMarkers: string[] = [];
  const flushMarkers = () => {
    if (pendingMarkers.length > 0) rows.push({ kind: 'markers', key: `markers:${pendingMarkers[0] ?? ''}`, ids: pendingMarkers });
    pendingMarkers = [];
  };

  order.forEach((id, index) => {
    const item = items.get(id);
    if (item === undefined) return;
    const preceding = sideBefore.get(index);
    if (preceding !== undefined) { flushMarkers(); rows.push(...preceding); }
    // Items of a sub-agent's stream are not in the order; the order is the main stream.
    placeDivider(item);

    // A failed adoption has no historical items yet. Its before-Orch band still explains the missing record.
    const missingHistory = item.kind === 'marker' && item.payload.marker === 'adopted' && item.payload.data.history_missing !== null;
    if (!beforePlaced && (item.before_orch || missingHistory)) {
      flushMarkers();
      beforePlaced = true;
      rows.push({ kind: 'before', key: 'before-orch' });
    }

    if (item.kind === 'marker' && (item.payload.marker === 'side_started' || item.payload.marker === 'side_closed')) {
      flushMarkers();
      // A matching close is part of the source row. A window with only the close still says what ended.
      if (item.payload.marker === 'side_closed' && !starts.has(item.payload.data.side.id)) rows.push({ kind: 'side', key: id, id });
    } else if (item.kind === 'marker' && item.payload.marker === 'orch_context') {
      // A turn owns its context link in the footer. A session context before the first turn
      // still stands as work detail when ordinary history lines are hidden.
      flushMarkers();
      if (item.turn_id === null) rows.push({ kind: 'item', key: id, id });
    } else if (item.kind === 'marker') {
      if (!options.hideHistory) {
        const last = pendingMarkers.length > 0 ? items.get(pendingMarkers[pendingMarkers.length - 1] ?? '') : undefined;
        const joins =
          last?.kind === 'marker' && last.payload.marker !== 'agent_history' && item.payload.marker !== 'agent_history' &&
          // Each goal outcome/action keeps its own evidence, reason and actor visible.
          !last.payload.marker.startsWith('goal_') && !item.payload.marker.startsWith('goal_') &&
          markerAuthor(last) !== null &&
          markerAuthor(last) === markerAuthor(item) &&
          Date.parse(item.created_at) - Date.parse(last.created_at) <= MARKERS_JOINED_WITHIN_MS;
        if (!joins) flushMarkers();
        pendingMarkers.push(id);
      }
    } else {
      flushMarkers();
      if (hiddenFailureItem(view, item)) {
        // Its footer, or the final native row of this failure, owns the one error line.
      } else if (isWork(item) && item.turn_id !== null && folded(item.turn_id)) {
        if (!foldPlaced.has(item.turn_id)) {
          foldPlaced.add(item.turn_id);
          const open = options.foldOpened.has(item.turn_id);
          rows.push({ kind: 'fold', key: `fold:${item.turn_id}`, turnId: item.turn_id, ids: steps.get(item.turn_id) ?? [], open });
        }
        // An opened fold shows its steps under the line, each as it would stand alone.
        if (options.foldOpened.has(item.turn_id)) rows.push({ kind: 'item', key: id, id });
      } else {
        // The first thing the agent says in a turn is preceded by its mark and name.
        const head = item.kind === 'agent_message' && (item.turn_id === null || !spoken.has(item.turn_id));
        if (item.kind === 'agent_message' && item.turn_id !== null) spoken.add(item.turn_id);
        rows.push(head ? { kind: 'item', key: id, id, head: true } : { kind: 'item', key: id, id });
      }
    }

    // A turn that is over ends in its footer, after the last thing it made.
    if (item.turn_id !== null && lastOfTurn.get(item.turn_id)?.last === index) {
      const turn = turns.get(item.turn_id);
      if (turn !== undefined && (isOver(turn) || contextTurns.has(turn.id))) {
        flushMarkers();
        const inputItemId=lastOfTurn.get(item.turn_id)?.inputItemId;
        rows.push({ kind: 'footer', key: `footer:${item.turn_id}`, turnId: item.turn_id,...(inputItemId===undefined?{}:{inputItemId}) });
      }
    }
    const following = sideAfter.get(index);
    if (following !== undefined) { flushMarkers(); rows.push(...following); }
  });
  flushMarkers();

  if (hasStatusLine(view.thread, view)) rows.push({ kind: 'status', key: 'status' });
  return rows;
}

/** Every waiting input, including the first, delivering and held rows, belongs to the one queue. */
export function queuedInputs(view: ThreadView): ThreadInput[] {
  // A delivered message can precede its input-state frame. Its input identity keeps
  // that stale row out of the queue, including steering and file handoffs.
  const spoken = new Map<string, { seq: number; status: Item['status'] }>();
  for (const item of view.items.values()) if (item.stream === 'main' && item.kind === 'user_message' && typeof item.payload.input_id === 'string') {
    const previous = spoken.get(item.payload.input_id);
    if (previous === undefined || previous.seq < item.seq_updated) spoken.set(item.payload.input_id, { seq: item.seq_updated, status: item.status });
  }
  return waitingInputs(view).filter((input) => {
    const message = spoken.get(input.id);
    // An unconfirmed reserved message can return to the queue. A completed
    // message still wins over a stale pending frame, even if that frame is newer.
    return message === undefined || (input.state === 'pending' && message.status !== 'completed' && input.seq_updated > message.seq);
  });
}

/**
 * The messages sent and not answered, less those the system has already told of: an input that carries the same client id is that
 * message, held now, and it is drawn as the input, not a second time as the message still on its way.
 */
function unannounced(view: ThreadView, sending: readonly SendingInput[]): readonly SendingInput[] {
  if (sending.length === 0) return sending;
  const told = new Set<string>();
  for (const input of view.inputs.values()) if (input.client_id !== null) told.add(input.client_id);
  return told.size === 0 ? sending : sending.filter((entry) => !told.has(entry.clientId));
}

/** Local attempts stay with their original execution history until its server input replaces them. */
export function sendingInTray(view: ThreadView, sending: readonly SendingInput[]): readonly SendingInput[] {
  const continuation = view.read?.projection?.viewed_continuation_id;
  const owned = sending.filter((entry) => entry.continuationId === undefined || entry.continuationId === null
    ? view.read?.selection === undefined || view.read.selection === null : entry.continuationId === continuation);
  return unannounced(view, owned);
}

/** What a row is taken to measure before it has been drawn, in pixels. */
export function estimateRow(row: Row, items: ReadonlyMap<string, Item>): number {
  switch (row.kind) {
    case 'divider':
    case 'status':
    case 'markers':
      return 32;
    case 'before':
      return 56;
    case 'side':
      // A folded aside is an ordinary control row; open lanes are measured by the list.
      return 36;
    case 'footer':
      return 28;
    case 'fold':
      return 36;
    case 'item': {
      const item = items.get(row.id);
      switch (item?.kind) {
        case 'user_message':
          return 80;
        case 'agent_message':
          return 24 + Math.min(40, Math.ceil(item.payload.text.length / 90)) * 24;
        case 'tool_call':
          return 40;
        case 'reasoning':
          return 32;
        case 'approval':
          return 150;
        default:
          return 56;
      }
    }
  }
}
