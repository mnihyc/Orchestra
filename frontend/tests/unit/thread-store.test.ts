/** What a thread keeps as rows and pieces arrive: a row is replaced by a newer one only, nothing is held twice, pieces wait for their item, a piece beyond what is held asks for the item again. */
import { describe, expect, it } from 'vitest';

import type { Item, ThreadInput, Turn } from '@/generated/api-types';
import { codePoints } from '@/realtime/deltas';
import { applyDeltas, applyRows, comparePositions, EMPTY_VIEW, runningTurn, type ThreadView, viewFromRows } from '@/realtime/stores/thread';

import { agentMessage, command, threadInput, turn, userMessage } from '../doubles/thread-rows';
import { ACME, me, userRef } from '../doubles/fixtures';
import { World } from '../doubles/world';

const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));

/** An item as the server numbered it: made at one sequence and last changed at another. */
const numbered = <T extends Item>(item: T, created: number, updated: number = created): T => ({ ...item, seq_created: created, seq_updated: updated });
const turnAt = (row: Turn, updated: number): Turn => ({ ...row, seq_updated: updated });
const inputAt = (row: ThreadInput, updated: number): ThreadInput => ({ ...row, seq_updated: updated });

const textOf = (view: ThreadView, id: string): string => {
  const item = view.items.get(id);
  return item?.kind === 'agent_message' ? item.payload.text : '';
};

const loaded = (items: Item[]): ThreadView => viewFromRows(EMPTY_VIEW, { items }, false);

describe('a thread as rows arrive', () => {
  it('ends the running indicator from the live thread row without waiting for an old turn row or a refresh', () => {
    const world = new World();
    const person = world.people.get('ada');
    const row = world.places.rowOf(ACME.mainThread);
    if (person === undefined || row === undefined) throw new Error('The fixture needs a complete native thread');
    const native = world.threads.thread(person, row);
    const old = turnAt(turn('old-turn', { ordinal: 1, state: 'running', inputId: 'first' }), 4);
    const current = turnAt(turn('current-turn', { ordinal: 2, state: 'running', inputId: 'second' }), 8);
    const running = viewFromRows(EMPTY_VIEW, { thread: { ...native, state: 'running', current_turn_id: current.id, seq: 8 }, turns: [old, current] }, true);
    expect(runningTurn(running)?.id).toBe(current.id);
    const idle = applyRows(running, { thread: { ...native, state: 'idle', current_turn_id: null, seq: 9 } });
    expect(runningTurn(idle)).toBeNull();
    // A delayed terminal turn or replayed older thread cannot bring back the spinner.
    const settled = applyRows(idle, { turns: [{ ...current, state: 'completed', seq_updated: 9 }], thread: running.thread });
    expect(runningTurn(settled)).toBeNull();
    expect(settled.turns.get(old.id)?.state).toBe('running');
    // A new pointer whose turn has not arrived must not choose another turn from history.
    const starting = applyRows(settled, { thread: { ...native, state: 'starting', current_turn_id: 'next-turn', seq: 10 } });
    expect(runningTurn(starting)).toBeNull();
  });

  it('keeps previously requested historical references when a bounded tail replaces its item window', () => {
    const historical = turnAt(turn('history-turn', { ordinal: 1, state: 'completed', inputId: 'first' }), 20);
    const previous = viewFromRows(EMPTY_VIEW, { turns: [historical], items: [numbered(agentMessage('old', 'Older answer', { turn: historical.id }), 1)] }, false);
    const current = turnAt(turn('new-turn', { ordinal: 128, state: 'running', inputId: 'latest' }), 30);
    const next = viewFromRows(previous, { turns: [current], items: [numbered(agentMessage('new', 'Current answer', { turn: current.id }), 29)] }, true);
    expect(next.items.has('old')).toBe(false);
    expect(next.turns.get(historical.id)).toEqual(historical);
    expect(next.turns.get(current.id)).toEqual(current);
    const stale = applyRows(next, { turns: [{ ...historical, state: 'running', seq_updated: 2 }] });
    expect(stale.turns.get(historical.id)?.state).toBe('completed');
  });

  it('holds a row once, however often it comes', () => {
    const rows = { items: [numbered(userMessage('u1', 'Hi', { author: ADA }), 1)] };
    const first = applyRows(EMPTY_VIEW, rows);
    expect(first.order).toEqual(['u1']);
    // The same row again changes nothing: not even the identity of the view.
    expect(applyRows(first, rows)).toBe(first);
    expect(applyRows(first, { items: [...rows.items, ...rows.items] })).toBe(first);
  });

  it('replaces a row by a newer one and never by an older one', () => {
    const first = applyRows(EMPTY_VIEW, { items: [numbered(agentMessage('a', 'one'), 1, 4)] });
    const older = applyRows(first, { items: [numbered(agentMessage('a', 'zero'), 1, 3)] });
    expect(textOf(older, 'a')).toBe('one');
    const newer = applyRows(first, { items: [numbered(agentMessage('a', 'two'), 1, 5)] });
    expect(textOf(newer, 'a')).toBe('two');
    expect(newer.order).toEqual(['a']);
  });

  it('keeps the conversation in the order the items were made, whatever order the rows come in', () => {
    const view = applyRows(EMPTY_VIEW, {
      items: [numbered(agentMessage('c', 'third'), 30), numbered(agentMessage('a', 'first'), 10), numbered(agentMessage('b', 'second'), 20)],
    });
    expect(view.order).toEqual(['a', 'b', 'c']);
    // A late row for an item in the middle takes its place there.
    const later = applyRows(view, { items: [numbered(agentMessage('bb', 'between'), 25)] });
    expect(later.order).toEqual(['a', 'b', 'bb', 'c']);
  });

  it('keeps the items of a sub-agent out of the conversation', () => {
    const view = applyRows(EMPTY_VIEW, {
      items: [numbered(agentMessage('m', 'main'), 1), numbered(agentMessage('s', 'side', { stream: 'sub-1' }), 2)],
    });
    expect(view.order).toEqual(['m']);
    expect(view.streams.get('sub-1')).toEqual(['s']);
    expect(view.items.has('s')).toBe(true);
  });

  it('replaces turns and inputs by their newer rows alone', () => {
    const t1 = turnAt(turn('t1', { ordinal: 1, state: 'running', inputId: 'i1' }), 5);
    const done = turnAt(turn('t1', { ordinal: 1, state: 'completed', inputId: 'i1' }), 8);
    const view = applyRows(EMPTY_VIEW, { turns: [done] });
    expect(applyRows(view, { turns: [t1] }).turns.get('t1')?.state).toBe('completed');
    const waiting = inputAt(threadInput('i1', 'Hi', { author: ADA, position: '1' }), 2);
    const delivered = inputAt(threadInput('i1', 'Hi', { author: ADA, position: '1', state: 'delivered' }), 6);
    const inputs = applyRows(applyRows(EMPTY_VIEW, { inputs: [delivered] }), { inputs: [waiting] });
    expect(inputs.inputs.get('i1')?.state).toBe('delivered');
  });

  it('loads anew from a whole answer, and keeps where the person had read when it was first loaded', () => {
    const first = viewFromRows(EMPTY_VIEW, { items: [numbered(agentMessage('a', 'x'), 1)] }, true);
    expect(first.loaded).toBe('tail');
    expect(first.hasOlder).toBe(true);
    const withRead: ThreadView = { ...first, openedReadSeq: 7, openedSeq: 8 };
    const again = viewFromRows(withRead, { items: [numbered(agentMessage('b', 'y'), 9)] }, false);
    expect(again.openedReadSeq).toBe(7);
    expect(again.openedSeq).toBe(8);
    // What was held is not carried over: the answer is the whole.
    expect(again.order).toEqual(['b']);
  });
});

describe('growing text in a thread', () => {
  const growing = (): ThreadView => loaded([numbered(agentMessage('a', 'Hello ', { status: 'in_progress' }), 1, 1)]);
  const piece = (offset: number, data: string) => ({ item_id: 'a', field: 'text' as const, offset, len: codePoints(data), data });

  it('is joined to its item, piece by piece, and a piece that came twice adds nothing', () => {
    let view = applyDeltas(growing(), [piece(6, 'wor')]).view;
    view = applyDeltas(view, [piece(9, 'ld')]).view;
    expect(textOf(view, 'a')).toBe('Hello world');
    const twice = applyDeltas(view, [piece(6, 'wor'), piece(9, 'ld')]);
    expect(textOf(twice.view, 'a')).toBe('Hello world');
    expect(twice.gaps).toEqual([]);
  });

  it('is joined into one change when a frame carries pieces that follow one another', () => {
    const result = applyDeltas(growing(), [piece(6, 'wor'), piece(9, 'ld'), piece(11, '!')]);
    expect(textOf(result.view, 'a')).toBe('Hello world!');
    expect(result.view.applied.get('a')?.text).toBe(12);
  });

  it('waits for its item when the item is not held yet, and goes in when the item arrives', () => {
    const waiting = applyDeltas(EMPTY_VIEW, [piece(0, 'Hel')]);
    expect(waiting.view.held.get('a')).toHaveLength(1);
    expect(waiting.gaps).toEqual([]);
    // The row comes with the text it had when it was written; the piece goes on from there.
    const arrived = applyRows(waiting.view, { items: [numbered(agentMessage('a', '', { status: 'in_progress' }), 1)] });
    expect(textOf(arrived, 'a')).toBe('Hel');
    expect(arrived.held.has('a')).toBe(false);
  });

  it('asks for the item again when a piece begins beyond what is held, and holds what follows until it is back', () => {
    const first = applyDeltas(growing(), [piece(12, 'far')]);
    expect(first.gaps).toEqual(['a']);
    expect(textOf(first.view, 'a')).toBe('Hello ');
    // More pieces while the item is read again are kept, not applied, and asked for nothing more.
    const second = applyDeltas(first.view, [piece(15, ' away')]);
    expect(second.gaps).toEqual([]);
    expect(textOf(second.view, 'a')).toBe('Hello ');
    expect([...(second.view.held.get('a') ?? [])].map((held) => held.data).join('')).toBe('far away');
    // The item as the server has it now reaches the pieces: they follow at once.
    const back = applyRows(second.view, { items: [numbered(agentMessage('a', 'Hello world ', { status: 'in_progress' }), 1, 5)] });
    expect(textOf(back, 'a')).toBe('Hello world far away');
    expect(back.held.has('a')).toBe(false);
    expect(back.gaps.has('a')).toBe(false);
  });

  it('keeps the end of a command’s output while it runs', () => {
    const view = loaded([numbered(command('c', { command: 'npm test', output: 'one\n', exit: null, status: 'in_progress' }), 1)]);
    const next = applyDeltas(view, [{ item_id: 'c', field: 'output', offset: 4, len: 4, data: 'two\n' }]).view;
    const item = next.items.get('c');
    expect(item?.kind === 'tool_call' && item.payload.tool.kind === 'command' ? item.payload.tool.output.inline : '').toBe('one\ntwo\n');
  });
});

describe('the place of an input in the queue', () => {
  it('is compared as a decimal number of any length', () => {
    expect(comparePositions('1', '2')).toBeLessThan(0);
    expect(comparePositions('10', '9')).toBeGreaterThan(0);
    expect(comparePositions('2.5', '2.25')).toBeGreaterThan(0);
    expect(comparePositions('2.5', '2.50')).toBe(0);
    expect(comparePositions('007', '7')).toBe(0);
    expect(comparePositions('123456789012345678901', '123456789012345678902')).toBeLessThan(0);
  });
});
