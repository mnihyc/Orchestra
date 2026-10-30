/** What the conversation is drawn as: which items stand alone and which fold into one line, by the depth the reader chose; lines of the thread's history that one person made together; the "New" line; a turn's footer; and what waits at the end. */
import { describe, expect, it } from 'vitest';

import type { Item, Thread, ThreadInput, Turn } from '@/generated/api-types';
import type { SendingInput } from '@/features/composer/Draft';
import { buildRows, foldsByDepth, queuedInputs, recentTurns, type Row, type RowOptions, sendingInTray } from '@/features/timeline/rows';
import { defaultThreadRead, newThreadReadScope } from '@/realtime/threadScope';
import { A, B, projection } from './continuation-fixtures';
import { applyRows, EMPTY_VIEW, type ThreadView, viewFromRows } from '@/realtime/stores/thread';

import { me, userRef } from '../doubles/fixtures';
import { approvalItem, harnessApproval, otherTool, agentMessage, command, fileChange, change, marker, plan, reasoning, threadInput, turn, userMessage } from '../doubles/thread-rows';

const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));
const GRACE = userRef(me(2, 'grace', 'Grace Hopper'));

const T0 = Date.parse('2026-10-01T10:00:00.000Z');
const at = (seconds: number): string => new Date(T0 + seconds * 1_000).toISOString();

/** Numbers items in the order given, as the server does. */
function made(items: Item[]): Item[] {
  return items.map((item, index) => ({ ...item, seq_created: index + 1, seq_updated: index + 1 }));
}

const thread = (more: Partial<Thread> = {}): Thread => ({ seq: 100, state: 'idle', lock: null, my_last_read_seq: null, ...more }) as unknown as Thread;

function viewOf(items: Item[], turns: Turn[] = [], inputs: ThreadInput[] = [], more: Partial<Thread> | null = null): ThreadView {
  const base = viewFromRows(EMPTY_VIEW, { items: made(items), turns, inputs, thread: more === null ? null : thread(more) }, false);
  return base;
}

const options = (more: Partial<RowOptions> = {}): RowOptions => ({ detail: 'steps', hideHistory: false, foldOpened: new Set(), meId: null, ...more });

const kinds = (rows: Row[]): string[] => rows.map((row) => row.kind);

it('keeps the live recovery row after a failed turn while other blockers remain in their banner', () => {
  const items = [userMessage('recover-user', 'Keep the work', { author: ADA, turn: 'recover-turn' })];
  const turns = [turn('recover-turn', { ordinal: 1, state: 'failed', inputId: 'recover-input' })];
  const recovering = viewOf(items, turns, [], { state: 'blocked', current_turn_id: null, blocked: { reason: 'error', detail: 'call_recovery' } });
  expect(buildRows(recovering, options()).at(-1)?.kind).toBe('status');
  const ordinary = viewOf(items, turns, [], { state: 'blocked', current_turn_id: null, blocked: { reason: 'attachments', detail: null } });
  expect(buildRows(ordinary, options()).some((row) => row.kind === 'status')).toBe(false);
});

it('shows the before-Orch band for a failed adoption even when no earlier items exist and history lines are hidden', () => {
  const adopted = marker('adopted-1', { marker: 'adopted', data: { by: ADA, native_session_ref: 'native-1', turns: 0, items: 0, history_missing: 'unreadable', history_error: 'The native record is locked', session_model: null, model_reason: 'not_reported' } });
  const rows = buildRows(viewOf([adopted]), options({ hideHistory: true }));
  expect(rows.filter((row) => row.kind === 'before')).toHaveLength(1);
});

/** Two turns with work in each, and an answer: the first is over, the second is the last. */
function twoTurns(second: Turn['state'] = 'completed'): ThreadView {
  return viewOf(
    [
      userMessage('u1', 'Why?', { author: ADA, turn: 't1', at: at(0) }),
      command('c1', { command: 'rg redirect', output: 'x', exit: 0, turn: 't1', at: at(1) }),
      reasoning('r1', 'Looking', { turn: 't1', at: at(2) }),
      agentMessage('a1', 'Because.', { turn: 't1', at: at(3) }),
      userMessage('u2', 'Fix it', { author: ADA, turn: 't2', at: at(10) }),
      fileChange('e1', { changes: [change('src/a.ts', null)], turn: 't2', at: at(11) }),
      command('c2', { command: 'npm test', output: 'ok', exit: 0, turn: 't2', at: at(12) }),
      agentMessage('a2', 'Done.', { turn: 't2', at: at(13) }),
    ],
    [
      turn('t1', { ordinal: 1, state: 'completed', inputId: 'i1', startedAt: at(0), completedAt: at(3), tokens: 100 }),
      turn('t2', { ordinal: 2, state: second, inputId: 'i2', startedAt: at(10), completedAt: second === 'running' ? null : at(13), tokens: second === 'running' ? undefined : 200 }),
    ],
  );
}

describe('the work of a turn', () => {
  it('is shown step by step in the last turn and folded to one line in the ones before it, at the depth called Steps', () => {
    const rows = buildRows(twoTurns(), options());
    expect(rows.filter((row) => row.kind === 'fold').map((row) => row.turnId)).toEqual(['t1']);
    // The last turn's steps stand as items.
    expect(rows.some((row) => row.kind === 'item' && row.id === 'e1')).toBe(true);
    expect(rows.some((row) => row.kind === 'item' && row.id === 'c2')).toBe(true);
    expect(rows.some((row) => row.kind === 'item' && row.id === 'c1')).toBe(false);
    const fold = rows.find((row) => row.kind === 'fold');
    expect(fold?.kind === 'fold' && fold.ids).toEqual(['c1', 'r1']);
  });

  it('is folded in every turn at the depth called Summary, and in none at the depth called Everything', () => {
    const summary = buildRows(twoTurns(), options({ detail: 'summary' }));
    expect(summary.filter((row) => row.kind === 'fold')).toHaveLength(2);
    expect(summary.some((row) => row.kind === 'item' && (row.id === 'e1' || row.id === 'c2'))).toBe(false);
    const everything = buildRows(twoTurns(), options({ detail: 'everything' }));
    expect(everything.filter((row) => row.kind === 'fold')).toHaveLength(0);
    expect(everything.some((row) => row.kind === 'item' && row.id === 'c1')).toBe(true);
  });

  it('is shown under its line when the reader opened the fold by hand', () => {
    const rows = buildRows(twoTurns(), options({ foldOpened: new Set(['t1']) }));
    const ids = rows.filter((row) => row.kind === 'item').map((row) => row.id);
    expect(ids).toContain('c1');
    expect(ids).toContain('r1');
    const fold = rows.find((row) => row.kind === 'fold');
    expect(fold?.kind === 'fold' && fold.open).toBe(true);
  });

  it('is not folded when it is only thought and a plan: nothing of it was done', () => {
    const view = viewOf(
      [userMessage('u1', 'Think', { author: ADA, turn: 't1' }), reasoning('r1', 'Hmm', { turn: 't1' }), plan('p1', [{ text: 'x', status: 'pending' }], { turn: 't1' }), agentMessage('a1', 'Ok', { turn: 't1' })],
      [turn('t1', { ordinal: 1, state: 'completed', inputId: 'i1' }), turn('t2', { ordinal: 2, state: 'completed', inputId: 'i2' })],
    );
    const rows = buildRows(view, options({ detail: 'summary' }));
    expect(rows.some((row) => row.kind === 'fold')).toBe(false);
  });

  it('is told by recentTurns and foldsByDepth: the one that runs and the last that is over', () => {
    const turns = new Map<string, Turn>([
      ['t1', turn('t1', { ordinal: 1, state: 'completed', inputId: 'a' })],
      ['t2', turn('t2', { ordinal: 2, state: 'failed', inputId: 'b' })],
      ['t3', turn('t3', { ordinal: 3, state: 'running', inputId: 'c' })],
    ]);
    const recent = recentTurns(turns);
    expect([...recent].sort()).toEqual(['t2', 't3']);
    expect(foldsByDepth('steps', 't1', recent)).toBe(true);
    expect(foldsByDepth('steps', 't3', recent)).toBe(false);
    expect(foldsByDepth('summary', 't3', recent)).toBe(true);
    expect(foldsByDepth('everything', 't1', recent)).toBe(false);
  });
});

describe('the end of a turn', () => {
  it('keeps a session’s context link before the first turn when ordinary history is hidden', () => {
    const context = marker('session-context', { marker: 'orch_context', data: { model_input_id: 'session-record', purpose: 'session' } });
    for (const hideHistory of [false, true]) {
      expect(buildRows(viewOf([context]), options({ hideHistory }))).toEqual([{ kind: 'item', key: 'session-context', id: 'session-context' }]);
    }
  });

  it.each(['running', 'completed'] as const)('puts context in the turn footer without a separate history row (%s)', (state) => {
    const context = marker('context-1', { marker: 'orch_context', data: { model_input_id: 'record-1', purpose: 'input' } }, { turn: 't1' });
    const view = viewOf([userMessage('u1', 'Fix login', { author: ADA, turn: 't1' }), context],
      [turn('t1', { ordinal: 1, state, inputId: 'i1' })]);
    for (const hideHistory of [false, true]) {
      const rows = buildRows(view, options({ hideHistory }));
      expect(rows).toEqual([{ kind: 'item', key: 'u1', id: 'u1' }, { kind: 'footer', key: 'footer:t1', turnId: 't1',inputItemId:'u1' }]);
    }
  });

  it('is its footer, after the last thing it made, when the turn is over', () => {
    const rows = buildRows(twoTurns(), options());
    const footers = rows.filter((row) => row.kind === 'footer').map((row) => row.turnId);
    expect(footers).toEqual(['t1', 't2']);
    // The footer of the first turn stands after its answer, before the next message of the reader.
    const order = rows.map((row) => (row.kind === 'item' ? row.id : row.kind === 'footer' ? `footer:${row.turnId}` : row.kind));
    expect(order.indexOf('footer:t1')).toBeGreaterThan(order.indexOf('a1'));
    expect(order.indexOf('footer:t1')).toBeLessThan(order.indexOf('u2'));
  });

  it('is not there while the turn runs; what the thread is doing is the last line instead', () => {
    const view: ThreadView = applyRows(twoTurns('running'), { thread: thread({ state: 'running', current_turn_id: 't2' }) });
    const rows = buildRows(view, options());
    expect(rows.filter((row) => row.kind === 'footer').map((row) => row.turnId)).toEqual(['t1']);
    expect(rows.at(-1)?.kind).toBe('status');
  });

  it('puts the answer of the agent under its mark and name once for each turn', () => {
    const view = viewOf(
      [
        userMessage('u1', 'Go', { author: ADA, turn: 't1' }),
        agentMessage('a1', 'First thought', { turn: 't1', phase: 'commentary' }),
        command('c1', { command: 'ls', output: '', exit: 0, turn: 't1' }),
        agentMessage('a2', 'And the result', { turn: 't1' }),
      ],
      [turn('t1', { ordinal: 1, state: 'running', inputId: 'i1' })],
    );
    const rows = buildRows(view, options({ detail: 'everything' }));
    const heads = rows.filter((row) => row.kind === 'item' && row.head === true).map((row) => row.kind === 'item' && row.id);
    expect(heads).toEqual(['a1']);
  });
});

describe('the history of a thread', () => {
  const renamed = (id: string, by: typeof ADA, second: number): Item => marker(id, { marker: 'renamed', data: { by, from: 'a', to: 'b' } }, { at: at(second) });

  it('is one line when one person made it within a minute, with the others apart', () => {
    const view = viewOf([renamed('m1', ADA, 0), renamed('m2', ADA, 20), renamed('m3', GRACE, 25), renamed('m4', ADA, 200)]);
    const rows = buildRows(view, options());
    const lines = rows.filter((row) => row.kind === 'markers').map((row) => row.ids);
    expect(lines).toEqual([['m1', 'm2'], ['m3'], ['m4']]);
  });

  it('keeps adjacent goal actions by one person visible separately from each other and ordinary history', () => {
    const set = marker('goal-set', { marker: 'goal_set', data: { goal_id: 'goal-1', by: ADA, objective: 'Fix login', condition: 'the tests pass' } }, { at: at(1) });
    const paused = marker('goal-paused', { marker: 'goal_paused', data: { goal_id: 'goal-1', by: ADA, reason: 'person', detail: null } }, { at: at(2) });
    const view = viewOf([renamed('m1', ADA, 0), set, paused, renamed('m2', ADA, 3)]);
    expect(buildRows(view, options()).filter((row) => row.kind === 'markers').map((row) => row.ids))
      .toEqual([['m1'], ['goal-set'], ['goal-paused'], ['m2']]);
  });

  it('keeps each aside as its own lane row, apart from ordinary history lines', () => {
    const began = (id: string, second: number): Item =>
      marker(id, { marker: 'side_started', data: { by: ADA, from_turn: 0, side: { id: `side-${id}`, kind: 'thread', name: 'Side', redacted: false } } }, { at: at(second) });
    const view = viewOf([began('s1', 0), began('s2', 5), renamed('m1', ADA, 6)]);
    const rows = buildRows(view, options());
    expect(rows.filter((row) => row.kind === 'side').map((row) => row.id)).toEqual(['s1', 's2']);
    expect(rows.filter((row) => row.kind === 'markers').map((row) => row.ids)).toEqual([['m1']]);
  });

  const aside = (id: string, ordinal: number) => marker(id, { marker: 'side_started', data: { by: ADA, from_turn: ordinal,
    side: { id: `aside-${id}`, kind: 'thread', name: 'Why the redirect?', redacted: false } } });

  it.each(['steps', 'summary', 'everything'] as const)('places an aside under its source turn even when asked later, with history hidden at %s depth', (detail) => {
    const view = applyRows(twoTurns(), { items: [{ ...aside('aside-start', 1), seq_created: 30, seq_updated: 30 }] });
    const rows = buildRows(view, options({ detail, hideHistory: true }));
    const source = rows.findIndex((row) => row.kind === 'footer' && row.turnId === 't1');
    expect(rows[source + 1]).toEqual({ kind: 'side', key: 'aside-start', id: 'aside-start' });
    expect(rows[source + 2]).toMatchObject({ kind: 'item', id: 'u2' });
    expect(rows.filter((row) => row.key === 'aside-start')).toHaveLength(1);
  });

  it('places an aside taken before the first turn before that turn, preserving the recorded boundary', () => {
    const view = applyRows(twoTurns(), { items: [{ ...aside('early', 0), seq_created: 30, seq_updated: 30 }] });
    const rows = buildRows(view, options());
    expect(rows[0]).toEqual({ kind: 'side', key: 'early', id: 'early' });
    expect(rows[1]).toMatchObject({ kind: 'item', id: 'u1' });
  });

  it('keeps two asides taken from the same turn in their own creation order', () => {
    const view = applyRows(twoTurns(), { items: [{ ...aside('first-aside', 1), seq_created: 30, seq_updated: 30 }, { ...aside('second-aside', 1), seq_created: 31, seq_updated: 31 }] });
    const rows = buildRows(view, options({ hideHistory: true }));
    const source = rows.findIndex((row) => row.kind === 'footer' && row.turnId === 't1');
    expect(rows.slice(source + 1, source + 3).map((row) => row.key)).toEqual(['first-aside', 'second-aside']);
    expect(rows[source + 3]).toMatchObject({ kind: 'item', id: 'u2' });
  });

  it('settles the source lane once with its actual closing summary and retains an orphaned close', () => {
    const start = aside('old-side', 1);
    const ended = marker('side-end', { marker: 'side_closed', data: { side: { id: 'aside-old-side', kind: 'thread', name: 'Why the redirect?', redacted: false }, by: ADA, turns: 3, why: 'closed', into: null } });
    const view = applyRows(twoTurns(), { items: [{ ...start, seq_created: 30, seq_updated: 30 }, { ...ended, seq_created: 31, seq_updated: 31 }] });
    const rows = buildRows(view, options({ hideHistory: true }));
    expect(rows.filter((row) => row.kind === 'side')).toEqual([{ kind: 'side', key: 'old-side', id: 'old-side', closedId: 'side-end' }]);
    expect(buildRows(viewOf([ended]), options({ hideHistory: true }))).toEqual([{ kind: 'side', key: 'side-end', id: 'side-end' }]);
  });

  it('keeps an aside readable when its source lies outside the loaded history, then moves it when that turn is loaded', () => {
    const window = { ...viewOf([agentMessage('latest', 'Later work', { turn: 't2' }), aside('outside', 1)], [turn('t2', { ordinal: 2, state: 'completed', inputId: 'i2' })]), hasOlder: true };
    expect(buildRows(window, options()).find((row) => row.kind === 'side')).toMatchObject({ id: 'outside', sourceTurn: 1 });
    const early = viewOf([aside('before-all', 0), agentMessage('latest', 'Later work', { turn: 't2' })], [turn('t2', { ordinal: 2, state: 'completed', inputId: 'i2' })]);
    expect(buildRows({ ...early, hasOlder: true }, options()).find((row) => row.kind === 'side')).toMatchObject({ sourceTurn: 0 });
    const complete = applyRows(twoTurns(), { items: [{ ...aside('outside', 1), seq_created: 30, seq_updated: 30 }] });
    expect(buildRows(complete, options()).find((row) => row.kind === 'side')).not.toHaveProperty('sourceTurn');
  });

  it('is left out when the reader hides it', () => {
    const view = viewOf([renamed('m1', ADA, 0), userMessage('u1', 'Hi', { author: ADA })]);
    expect(kinds(buildRows(view, options({ hideHistory: true })))).toEqual(['item']);
  });
});

describe('the line that says where the new things begin', () => {
  it('stands before the first thing the reader had not seen, and never for what they wrote themselves', () => {
    const view = viewOf(
      [userMessage('u1', 'Old', { author: GRACE }), userMessage('u2', 'My own', { author: ADA }), agentMessage('a1', 'New to me')],
      [],
      [],
      { seq: 50, my_last_read_seq: 1 },
    );
    // Items were numbered 1, 2 and 3 in the order made; the reader had read to 1.
    const rows = buildRows({ ...view, openedReadSeq: 1, openedSeq: 3 }, options({ meId: ADA.id }));
    const order = rows.map((row) => (row.kind === 'item' ? row.id : row.kind));
    expect(order).toEqual(['u1', 'u2', 'divider', 'a1']);
  });

  it('is not drawn for what arrives while the reader watches', () => {
    // Opened with everything read up to 2; the third item came afterwards, as the reader looked.
    const view = viewOf([userMessage('u1', 'Old', { author: GRACE }), userMessage('u2', 'Older', { author: GRACE }), agentMessage('a1', 'Arrived live')], [], [], { seq: 3, my_last_read_seq: 2 });
    expect(buildRows({ ...view, openedReadSeq: 2, openedSeq: 2 }, options({ meId: ADA.id })).some((row) => row.kind === 'divider')).toBe(false);
  });

  it('is not drawn when the reader had read everything', () => {
    const view = viewOf([userMessage('u1', 'Old', { author: GRACE })], [], [], { seq: 1, my_last_read_seq: 1 });
    expect(buildRows({ ...view, openedReadSeq: 1, openedSeq: 1 }, options()).some((row) => row.kind === 'divider')).toBe(false);
  });
});

describe('the one queue above the conversation’s composer', () => {
  it('uses a main-stream message’s input identity to settle a stale queued row, including later stale input frames', () => {
    const one = { ...threadInput('i1', 'Guide this turn', { author: ADA, position: '1', delivery: 'steer', state: 'delivering' }), seq_updated: 4 };
    const two = { ...threadInput('i2', 'Same words', { author: ADA, position: '2' }), seq_updated: 4 };
    const child = userMessage('child', 'Guide this turn', { author: ADA, inputId: 'i1', stream: 'sub:reviewer' });
    const queued = viewOf([child], [], [one, two], { state: 'running' });
    expect(queuedInputs(queued).map((input) => input.id)).toEqual(['i1', 'i2']);
    const message = { ...userMessage('spoken', 'Guide this turn', { author: ADA, inputId: 'i1', delivery: 'steer' }), seq_created: 5, seq_updated: 5 };
    const spoken = applyRows(queued, { items: [message] });
    expect(queuedInputs(spoken).map((input) => input.id)).toEqual(['i2']);
    expect(spoken.inputs.get('i1')).toEqual(one);
    const replayed = applyRows(spoken, { inputs: [{ ...one, seq_updated: 6 }] });
    expect(queuedInputs(replayed).map((input) => input.id)).toEqual(['i2']);
    expect(replayed.items.get('spoken')).toEqual(message);
  });
  it('shows an input returned to the queue after its earlier message', () => {
    const spoken = { ...userMessage('spoken', 'Try again', { author: ADA, inputId: 'i1', status: 'in_progress' }), seq_created: 5, seq_updated: 5 };
    const stale = { ...threadInput('i1', 'Try again', { author: ADA, position: '1' }), seq_updated: 4 };
    const view = applyRows(viewOf([], [], [stale], { state: 'idle' }), { items: [spoken] });
    expect(queuedInputs(view)).toEqual([]);
    const returned = applyRows(view, { inputs: [{ ...stale, seq_updated: 6 }] });
    expect(queuedInputs(returned).map((input) => input.id)).toEqual(['i1']);
  });
  it('holds the first input too when idle, leaving no unsent grey row in the conversation', () => {
    const inputs = [
      threadInput('i1', 'First', { author: ADA, position: '1' }),
      threadInput('i2', 'Second', { author: ADA, position: '2' }),
      threadInput('i3', 'Third', { author: ADA, position: '3' }),
    ].map((input) => ({ ...input, seq_updated: 1 }));
    const view = viewOf([agentMessage('a1', 'x')], [], inputs, { state: 'idle' });
    const rows = buildRows(view, options());
    expect(kinds(rows)).toEqual(['item']);
    expect(queuedInputs(view).map((input) => input.id)).toEqual(['i1', 'i2', 'i3']);
  });

  it('is all in the tray while a turn runs, and nothing is drawn in the conversation', () => {
    const inputs = [threadInput('i1', 'First', { author: ADA, position: '1' }), threadInput('i2', 'Second', { author: ADA, position: '2' })].map((input) => ({ ...input, seq_updated: 1 }));
    const view = viewOf([userMessage('u0', 'Go', { author: ADA, turn: 't1' })], [turn('t1', { ordinal: 1, state: 'running', inputId: 'i0' })], inputs, { state: 'running', current_turn_id: 't1' });
    const rows = buildRows(view, options());
    expect(kinds(rows)).toEqual(['item', 'status']);
    expect(queuedInputs(view).map((input) => input.id)).toEqual(['i1', 'i2']);
  });

  it('holds every unacknowledged send, including the first, without a timeline duplicate', () => {
    const sending = [{ clientId: 'k1', content: [{ type: 'text' as const, text: 'Hello' }], delivery: 'queue' as const, state: 'sending' as const, at: at(0) }];
    const view = viewOf([agentMessage('a1', 'x')], [], [], { state: 'idle' });
    expect(kinds(buildRows(view, options()))).toEqual(['item']);
    expect(sendingInTray(view, sending)).toEqual(sending);
  });

  it('is drawn once when the system has told of a message that is still on its way: it is the input now', () => {
    const entry: SendingInput = { clientId: 'k1', content: [{ type: 'text', text: 'Hello' }], delivery: 'queue', state: 'sending', at: at(0) };
    const sending = [entry];
    const told = { ...threadInput('i1', 'Hello', { author: ADA, position: '1', clientId: 'k1' }), seq_updated: 1 };

    // A turn runs: the input stands in the tray, and the message on its way is not a second line there.
    const running = viewOf([userMessage('u0', 'Go', { author: ADA, turn: 't1' })], [turn('t1', { ordinal: 1, state: 'running', inputId: 'i0' })], [told], { state: 'running' });
    expect(queuedInputs(running).map((input) => input.id)).toEqual(['i1']);
    expect(sendingInTray(running, sending)).toEqual([]);

    // Nothing runs: the acknowledged input replaces the local send in the same queue.
    const idle = viewOf([agentMessage('a1', 'x')], [], [told], { state: 'idle' });
    const rows = buildRows(idle, options());
    expect(kinds(rows)).toEqual(['item']);
    expect(queuedInputs(idle).map((input) => input.id)).toEqual(['i1']);
    expect(sendingInTray(idle, sending)).toEqual([]);

    // The input went in at once and stands in the conversation as the person's message: the one on its way is not drawn after it.
    const delivered = { ...threadInput('i1', 'Hello', { author: ADA, position: '1', clientId: 'k1', state: 'delivered' }), seq_updated: 1 };
    const spoken = viewOf(
      [userMessage('u1', 'Hello', { author: ADA, turn: 't1', inputId: 'i1' }), agentMessage('a1', 'x', { turn: 't1' })],
      [turn('t1', { ordinal: 1, state: 'completed', inputId: 'i1' })],
      [delivered],
      { state: 'idle' },
    );
    expect(sendingInTray(spoken, sending)).toEqual([]);

    // A message the system has not told of stays as it was.
    const other = [{ ...entry, clientId: 'k2' }];
    expect(sendingInTray(running, other).map((entry) => entry.clientId)).toEqual(['k2']);
    expect(sendingInTray(spoken, other)).toEqual(other);
    expect(kinds(buildRows(spoken, options()))).toEqual(['item', 'item', 'footer']);
  });

  it('keeps Stop, caps, attachment holds and delivering inputs in stable order while blocked', () => {
    const rows = [threadInput('cap', 'Cap', { author: ADA, position: '2', detail: 'cap_reached' }),
      { ...threadInput('stop', 'Stop held', { author: ADA, position: '1' }), queue_held: true },
      threadInput('files', 'Files', { author: ADA, position: '3', detail: 'attachments' }),
      threadInput('delivering', 'On its way', { author: ADA, position: '4', state: 'delivering' }),
      threadInput('done', 'Already sent', { author: ADA, position: '5', state: 'delivered' })];
    const view = viewOf([], [], rows, { state: 'blocked', blocked: { reason: 'attachments', detail: null } });
    expect(queuedInputs(view).map((input) => input.id)).toEqual(['stop', 'cap', 'files', 'delivering']);
    expect(buildRows(view, options())).toEqual([]);
  });
});


it('keeps unacknowledged sends with their original history through active and retained browsing', () => {
  const entry: SendingInput = { clientId: 'origin-a', continuationId: A, content: [{ type: 'text', text: 'Keep old words' }],
    delivery: 'queue', state: 'not_sent', at: at(0) };
  const other = { ...entry, clientId: 'origin-b', continuationId: B };
  const sending = [entry, other];
  const base = viewOf([], [], [], { state: 'idle' });
  const live: ThreadView = { ...base, read: { ...newThreadReadScope(1, defaultThreadRead('native-thread')),
    projection: projection(B), ready: true, provisional: false } };
  const saved: ThreadView = { ...live, read: { ...live.read!, selection: A, projection: projection(A, B, 1, false) } };
  expect(sendingInTray(live, sending)).toEqual([other]);
  expect(sendingInTray(saved, sending)).toEqual([entry]);
  expect(sendingInTray(live, sending)).toEqual([other]);
  const acknowledged = { ...saved, inputs: new Map([['accepted', { ...threadInput('accepted', 'Keep old words',
    { author: ADA, position: '1', clientId: entry.clientId }), continuation_id: A }]]) };
  expect(sendingInTray(acknowledged, sending)).toEqual([]);
  expect(sending).toEqual([entry, other]); expect(entry.state).toBe('not_sent');
});


it.each(['steps', 'summary', 'everything'] as const)('shows one explicitly correlated question while retaining its native work (%s)', (detail) => {
  const source = otherTool('question-native', { name: 'AskUserQuestion', turn: 'question-turn', args: { questions: ['Which target?'] }, result: { answers: ['docs'] } });
  const card = approvalItem('question-card', 'question', { turn: 'question-turn' });
  if (card.kind !== 'approval') throw new Error('The fixture must be an approval item.');
  const approval = harnessApproval('question', { threadId: card.thread_id, itemId: card.id, requester: ADA, title: 'Which target?',
    request: { request_kind: 'user_input', detail: { questions: [{ id: 'target', header: 'Target', question: 'Which target?', multiple: false, allow_other: true, options: null }] }, options: [] } });
  const linked = { ...card, parent_item_id: source.id, payload: { ...card.payload, approval } };
  const view = viewOf([source, linked], [turn('question-turn', { ordinal: 1, state: 'running', inputId: 'question-input' })]);
  const rows = buildRows(view, options({ detail }));
  expect(rows.filter((row) => row.kind === 'item' && row.id === card.id)).toHaveLength(1);
  expect(rows.some((row) => row.kind === 'item' && row.id === source.id || row.kind === 'fold' && row.ids.includes(source.id))).toBe(false);
  expect(view.items.get(source.id)?.payload).toEqual(source.payload);
});

it('keeps permission tools and unrelated native work visible even when an approval has a parent', () => {
  const source = command('permission-native', { command: 'make', output: '', exit: 0, turn: 'permission-turn' });
  const card = approvalItem('permission-card', 'permission', { turn: 'permission-turn' });
  if (card.kind !== 'approval') throw new Error('The fixture must be an approval item.');
  const approval = harnessApproval('permission', { threadId: card.thread_id, itemId: card.id, requester: ADA, title: 'Run make?', request: { request_kind: 'command', options: [], detail: { command: 'make', cwd: '/workspace', reason: null } } });
  const linked = { ...card, parent_item_id: source.id, payload: { ...card.payload, approval } };
  const rows = buildRows(viewOf([source, linked], [turn('permission-turn', { ordinal: 1, state: 'running', inputId: 'permission-input' })]), options({ detail: 'everything' }));
  expect(rows.filter((row) => row.kind === 'item').map((row) => row.id)).toEqual([source.id, card.id]);
});
