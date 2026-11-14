/** The rows the conversation is drawn as: which items stand alone, which are drawn together as the work of their turn, and where the lines that belong to no item go.
 *
 * The items of a thread are in the order they were made. What is drawn is decided here, apart from
 * any screen, so that it can be tested by what a reader would see: in Normal reading everything the agent
 * did between a person's message and its answer (steps, thought, and the sentences said between them)
 * is one group with one line; in Verbose and Diagnostics reading every step is a row of its own.
 * Lines of the thread's history that one person made within a minute become one; the "New" line stands
 * before the first thing the reader has not seen; a turn that is over ends in its footer; the next
 * waiting inputs belong to the queue above the composer. What the thread is doing now is drawn
 * separately in the dock. The harness's own state (`rowKinds.ts`) is drawn only in Diagnostics
 * reading; the beginning of a thread is one line that opens into the lines it stands for.
 */
import { approvalSnapshot } from '@/features/approvals/snapshot';
import { isTurnAnswer } from '@/features/timeline/answers';
import type { SendingInput } from '@/features/composer/Draft';
import type { Item, ThreadInput, Turn } from '@/generated/api-types';
import { type ThreadView, waitingInputs } from '@/realtime/stores/thread';
import { hiddenFailureItem } from './failures';
import { hiddenConversationItems, inlineMarker, rowKind } from './rowKinds';

/** How a conversation is read: Normal folds the work of a turn into one line; Verbose draws every step; Diagnostics adds the program's own state. */
export type WorkDetail = 'normal' | 'verbose' | 'diagnostics';

/** Lines of the thread's history by one person this close together become one line with a count. */
export const MARKERS_JOINED_WITHIN_MS = 60_000;

export type Row =
  /** An item drawn on its own: a message, a step, a plan, an approval, an error. `answer`: the last thing the agent said in a turn that is over; `named`: a person's message in a thread that more than one person wrote in. */
  | { kind: 'item'; key: string; id: string; answer?: true; named?: true }
  /** The work of a turn between two things that are said: its steps and the sentences between them, drawn as one line that opens into them. `last`: no more work of the turn follows; `live`: the turn is still at it; `only`: it is all the work of its turn. */
  | { kind: 'work'; key: string; turnId: string; ids: readonly string[]; last: boolean; live: boolean; only: boolean }
  /** Lines of the thread's history, joined: the first is drawn with how many there were. */
  | { kind: 'markers'; key: string; ids: readonly string[] }
  /** An aside at the turn it was asked from; its closing marker settles the same row. */
  | { kind: 'side'; key: string; id: string; closedId?: string; sourceTurn?: number }
  /** The end of a turn that is over. `answered`: its answer carries the turn's acts; `lined`: a work line above says how it ended. */
  | { kind: 'footer'; key: string; turnId: string; inputItemId?: string; answered?: true; lined?: true }
  /** Where what the reader had not seen begins. */
  | { kind: 'divider'; key: string }
  /** Where what an adopted session held before Orch took it in begins: the heading of those items. */
  | { kind: 'before'; key: string }
  /** How the thread began: it was created, its source prepared, its agent started; drawn as one line that opens into those lines. */
  | { kind: 'start'; key: string; ids: readonly string[] };

export interface RowOptions {
  detail: WorkDetail;
  hideHistory: boolean;
  /** The person looking: what they wrote themselves is never "new" to them. */
  meId: string | null;
  /** Explicit navigation reveals hidden rows; ordinary work keeps its group, which the reveal path opens. */
  revealed?: ReadonlySet<string>;
}

/** What the agent did that is not a thing it says: a tool call, thought, a plan, or an approval that has been decided. */
export function isStep(item: Item): boolean {
  return item.kind === 'tool_call' || item.kind === 'reasoning' || item.kind === 'plan' || item.kind === 'approval' && item.status !== 'pending';
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

/** A managed service can wait for recovery after its model turn has ended. */
export function hasPausedService(view: ThreadView): boolean {
  return view.order.some((id) => {
    const item = view.items.get(id);
    if (item?.kind !== 'tool_call' || item.payload.tool.kind !== 'orch') return false;
    const tool = item.payload.tool;
    if (tool.tool !== 'web_search' && tool.tool !== 'web_fetch') return false;
    const result = tool.result;
    return result !== null && typeof result === 'object' && !Array.isArray(result) && 'state' in result && result.state === 'paused';
  });
}

/** One answer of an ended turn, and the text that stands outside its work while a reply is being written. */
export function classifyAgentMessages(view: Pick<ThreadView, 'items' | 'order' | 'turns'>): { answers: ReadonlyMap<string, string>; visible: ReadonlySet<string> } {
  const lastStep = new Map<string, number>();
  const final = new Map<string, string>();
  const latest = new Map<string, { item: Extract<Item, { kind: 'agent_message' }> }>();
  const visible = new Set<string>();
  view.order.forEach((id) => {
    const item = view.items.get(id);
    if (!item?.turn_id) return;
    if (isStep(item)) lastStep.set(item.turn_id, item.seq_created);
    if (item.kind !== 'agent_message' || item.payload.text.trim() === '') return;
    latest.set(item.turn_id, { item });
    if (item.payload.phase === 'final') { final.set(item.turn_id, id); visible.add(id); }
  });
  const answers = new Map<string, string>();
  for (const [turnId, last] of latest) {
    const turn = view.turns.get(turnId);
    if (turn !== undefined && isOver(turn)) {
      const selected = view.items.get(final.get(turnId) ?? last.item.id);
      const answer = selected?.kind === 'agent_message' && isTurnAnswer(selected.seq_created, selected.payload.phase, {
        ended: true, lastAgent: last.item.seq_created, lastFinal: final.has(turnId) ? selected.seq_created : null, lastWork: lastStep.get(turnId) ?? null,
      }) ? selected.id : null;
      if (answer !== null) { answers.set(turnId, answer); visible.add(answer); }
    } else if (last.item.seq_created > (lastStep.get(turnId) ?? -1)) visible.add(last.item.id);
  }
  return { answers, visible };
}

export function buildRows(view: ThreadView, options: RowOptions): Row[] {
  const { items, turns } = view;
  const grouped = options.detail === 'normal';
  const diagnostics = options.detail === 'diagnostics';
  const contentSources = new Set<string>();
  for (const item of items.values()) {
    if (item.kind !== 'approval' || item.parent_item_id === null) continue;
    const approval = approvalSnapshot(item.payload.approval, item.payload.approval_id);
    if (approval?.kind !== 'harness' || !['user_input', 'mcp_elicitation'].includes(approval.request.request_kind)) continue;
    const source = items.get(item.parent_item_id);
    if (source?.kind === 'tool_call' && source.thread_id === item.thread_id && source.turn_id === item.turn_id) contentSources.add(source.id);
  }
  // A content card owns its native work detail. Keep the item in the store, not a second question row.
  const order = view.order.filter((id) => !contentSources.has(id) || options.revealed?.has(id) === true);
  const rows: Row[] = [];

  const hiddenIds = hiddenConversationItems(order, items, { detail: options.detail, hideHistory: options.hideHistory });
  const drawsMarker = (item: Item): boolean => !options.hideHistory || inlineMarker(item) || diagnostics && rowKind(item) === 'diagnostics';

  // How the thread began, before its first turn: created, its source prepared, its agent started.
  const startIds: string[] = [];
  let started = false;
  if (!view.hasOlder) {
    for (const id of order) {
      const item = items.get(id);
      if (item === undefined) continue;
      if (item.kind === 'user_message' || (item.kind !== 'marker' && item.turn_id !== null)) break;
      if (item.kind !== 'marker') continue;
      const beginning = item.payload;
      if (beginning.marker === 'created' || beginning.marker === 'source_prepared') startIds.push(id);
      else if (beginning.marker === 'segment_started' && !beginning.data.resumed && !started) {
        started = true;
        startIds.push(id);
      }
    }
  }
  const startLines = started && !startIds.some((id) => options.revealed?.has(id) === true) ? new Set(startIds) : new Set<string>();

  // The last place each turn is seen in the order, and the input that began it; and whether more than one person wrote in
  // the thread, for only then does a person's message say who wrote it.
  const lastOfTurn = new Map<string, { last: number; inputItemId?: string }>();
  let sender: string | null = null;
  // A partial window cannot establish that only one person wrote in the thread. Keep attribution until that is known.
  let several = view.hasOlder || view.hasNewer;
  order.forEach((id, index) => {
    const item = items.get(id);
    if (item?.kind === 'user_message' && item.payload.source === 'user' && item.author !== null) {
      if (sender === null) sender = item.author.id;
      else if (sender !== item.author.id) several = true;
    }
    const turnId = item?.turn_id ?? null;
    if (item === undefined || turnId === null) return;
    let entry = lastOfTurn.get(turnId);
    if (entry === undefined) { entry = { last: index }; lastOfTurn.set(turnId, entry); } else entry.last = index;
    if (item.kind === 'user_message' && entry.inputItemId === undefined) entry.inputItemId = item.id;
  });

  // Explicit commentary stays work after a turn ends. Unmarked programs contribute only their last trailing message as the answer.
  const { answers, visible: replies } = classifyAgentMessages({ ...view, order });
  const members = new Set<string>();
  const lastMember = new Map<string, string>();
  for (const id of order) {
    const item = items.get(id);
    if (item === undefined || (hiddenIds.has(id) && options.revealed?.has(id) !== true) || item.turn_id === null) continue;
    if (isStep(item) || item.kind === 'agent_message' && item.payload.text.trim() !== '' && !replies.has(id)) {
      members.add(id);
      lastMember.set(item.turn_id, id);
    }
  }

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

  // The work of a turn is drawn as one group: the steps and the sentences between them that follow one another stand
  // as one row, which anything else drawn in between (something said to the person, a line of history, the "New" line,
  // a message steered in) ends. A question or an approval that waits for a person stands under the group it came from.
  const worked = new Set<string>();
  let run: { turnId: string; ids: string[]; lifted: Row[] } | null = null;
  const closeRun = () => {
    const done = run;
    run = null;
    if (done === null) return;
    const turn = turns.get(done.turnId);
    const last = lastMember.get(done.turnId) === done.ids[done.ids.length - 1];
    const live = last && turn !== undefined && (turn.state === 'running' || turn.state === 'starting');
    worked.add(done.turnId);
    rows.push({ kind: 'work', key: `work:${done.ids[0] ?? ''}`, turnId: done.turnId, ids: done.ids, last, live, only: true });
    rows.push(...done.lifted);
  };
  const push = (...more: Row[]) => {
    closeRun();
    rows.push(...more);
  };

  // What the reader had not seen when they opened the thread; what arrived since they watched arrive.
  const dividerAfter = view.openedReadSeq !== null && view.openedSeq !== null && view.openedReadSeq < view.openedSeq ? view.openedReadSeq : null;
  const dividerUntil = view.openedSeq ?? 0;
  let dividerPlaced = dividerAfter === null;
  const placeDivider = (item: Item) => {
    if (dividerPlaced || dividerAfter === null) return;
    if (item.seq_created > dividerAfter && item.seq_created <= dividerUntil && item.author?.id !== options.meId) {
      push({ kind: 'divider', key: 'divider' });
      dividerPlaced = true;
    }
  };

  // What a session held before it was adopted stands under one heading, where the first of it stands.
  let beforePlaced = false;
  let pendingMarkers: string[] = [];
  const flushMarkers = () => {
    if (pendingMarkers.length > 0) {
      closeRun();
      rows.push({ kind: 'markers', key: `markers:${pendingMarkers[0] ?? ''}`, ids: pendingMarkers });
    }
    pendingMarkers = [];
  };

  order.forEach((id, index) => {
    const item = items.get(id);
    if (item === undefined) return;
    const preceding = sideBefore.get(index);
    if (preceding !== undefined) { flushMarkers(); push(...preceding); }
    // What stays out of the reading (the harness's own state, an answer said twice) leaves no line, no divider and no heading.
    const requested = options.revealed?.has(id) === true;
    const hidden = hiddenIds.has(id) && !requested;
    // Items of a sub-agent's stream are not in the order; the order is the main stream.
    if (!hidden) placeDivider(item);

    // A failed adoption has no historical items yet. Its before-Orch band still explains the missing record.
    const missingHistory = item.kind === 'marker' && item.payload.marker === 'adopted' && item.payload.data.history_missing !== null;
    if (!beforePlaced && (missingHistory || !hidden && item.before_orch)) {
      flushMarkers();
      beforePlaced = true;
      push({ kind: 'before', key: 'before-orch' });
    }

    if (requested && (hiddenIds.has(id) || item.kind === 'marker') && !(item.kind === 'marker' && (item.payload.marker === 'side_started' || item.payload.marker === 'side_closed'))) {
      flushMarkers();
      push({ kind: 'item', key: id, id });
    } else if (startLines.has(id)) {
      if (options.hideHistory) {
        // History lines are not drawn.
      } else if (diagnostics) {
        // Where the reader asked for everything, each line of the beginning stands as it was made.
        flushMarkers();
        push({ kind: 'markers', key: `markers:${id}`, ids: [id] });
      } else if (id === startIds[0]) {
        // The lines of the beginning stand in one line, where the first of them stands.
        flushMarkers();
        push({ kind: 'start', key: 'start', ids: startIds });
      }
    } else if (hidden) {
      // Kept as it was, and drawn where the reader asked for it.
    } else if (item.kind === 'marker' && (item.payload.marker === 'side_started' || item.payload.marker === 'side_closed')) {
      flushMarkers();
      // A matching close is part of the source row. A window with only the close still says what ended.
      if (item.payload.marker === 'side_closed' && !starts.has(item.payload.data.side.id)) push({ kind: 'side', key: id, id });
    } else if (item.kind === 'marker' && item.payload.marker === 'orch_context') {
      // A turn owns its context link in the footer. A session context before the first turn
      // still stands as work detail when ordinary history lines are hidden.
      flushMarkers();
      if (item.turn_id === null) push({ kind: 'item', key: id, id });
    } else if (item.kind === 'marker' && item.payload.marker === 'background_wake') {
      // A turn's recorded cause precedes its output even when ordinary lifecycle history is hidden.
      flushMarkers();
      push({ kind: 'item', key: id, id });
    } else if (item.kind === 'marker' && item.payload.marker === 'harness_changed' && rowKind(item) === 'conversation') {
      // Warnings and failures explain this answer even when lifecycle history is hidden.
      flushMarkers();
      push({ kind: 'item', key: id, id });
    } else if (item.kind === 'marker') {
      if (drawsMarker(item)) {
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
      if (!diagnostics && hiddenFailureItem(view, item)) {
        // Its footer, or the final native row of this failure, owns the one error line.
      } else if (grouped && item.turn_id !== null && members.has(id)) {
        if (run?.turnId !== item.turn_id) {
          closeRun();
          run = { turnId: item.turn_id, ids: [], lifted: [] };
        }
        run.ids.push(id);
      } else if (grouped && item.kind === 'approval' && item.status === 'pending' && run !== null && run.turnId === item.turn_id) {
        // Waiting for a person: it stands whole under the group until it is decided, and then it is one of its steps.
        run.lifted.push({ kind: 'item', key: id, id });
      } else {
        const turn = item.turn_id === null ? undefined : turns.get(item.turn_id);
        const answer = item.turn_id !== null && answers.get(item.turn_id) === id && turn !== undefined && isOver(turn);
        const named = several && item.kind === 'user_message' && item.payload.source === 'user';
        push({ kind: 'item', key: id, id, ...(answer ? { answer: true as const } : {}), ...(named ? { named: true as const } : {}) });
      }
    }

    // A turn that is over ends in its footer, after the last thing it made.
    if (item.turn_id !== null && lastOfTurn.get(item.turn_id)?.last === index) {
      const turn = turns.get(item.turn_id);
      if (turn !== undefined && isOver(turn)) {
        flushMarkers();
        const inputItemId = lastOfTurn.get(item.turn_id)?.inputItemId;
        push({ kind: 'footer', key: `footer:${item.turn_id}`, turnId: item.turn_id, ...(inputItemId === undefined ? {} : { inputItemId }),
          ...(answers.has(item.turn_id) ? { answered: true as const } : {}), ...(worked.has(item.turn_id) || run?.turnId === item.turn_id ? { lined: true as const } : {}) });
      }
    }
    const following = sideAfter.get(index);
    if (following !== undefined) { flushMarkers(); push(...following); }
  });
  flushMarkers();
  closeRun();

  // A turn whose work was cut into several groups (a message steered in between) has no one time of its own for any of them.
  const groups = new Map<string, number>();
  for (const row of rows) if (row.kind === 'work') groups.set(row.turnId, (groups.get(row.turnId) ?? 0) + 1);
  return rows.map((row) => row.kind === 'work' && groups.get(row.turnId) !== 1 ? { ...row, only: false } : row);
}

/** Every waiting input, including the first, delivering and held rows, belongs to the one queue. */
export function queuedInputs(view: ThreadView): ThreadInput[] {
  // A delivered message can precede its input-state frame. Its native input id is proof that
  // this same row is already in the conversation, including steering and file handoffs.
  const spoken = new Set<string>();
  for (const item of view.items.values()) if (item.stream === 'main' && item.kind === 'user_message' && typeof item.payload.input_id === 'string') spoken.add(item.payload.input_id);
  return waitingInputs(view).filter((input) => !spoken.has(input.id));
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
    case 'markers':
      return 32;
    case 'before':
      return 56;
    case 'start':
      return 32;
    case 'side':
      // A folded aside is an ordinary control row; open lanes are measured by the list.
      return 36;
    case 'footer':
      return 48;
    case 'work':
      // The line alone, or the line over the bounded preview of a turn that is still at it.
      return row.live ? 280 : 40;
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
