/** The growing text of a thread: pieces that arrive in order, twice, late and out of place, and how a row that arrives joins what is held. */
import { describe, expect, it } from 'vitest';

import {
  appliedOf,
  applyDelta,
  codePoints,
  type Delta,
  dropCodePoints,
  joinRow,
  mergeDeltas,
  OUTPUT_TAIL_CHARS,
} from '@/realtime/deltas';

import { agentMessage, command, reasoning } from '../doubles/thread-rows';

const piece = (itemId: string, field: Delta['field'], offset: number, data: string, len: number = codePoints(data)): Delta => ({ item_id: itemId, field, offset, len, data });

const textOf = (item: ReturnType<typeof agentMessage>): string => (item.kind === 'agent_message' ? item.payload.text : '');

describe('the length of a text', () => {
  it('is counted in code points, so that one character outside the basic plane is one', () => {
    expect(codePoints('abc')).toBe(3);
    expect(codePoints('a😀b')).toBe(3);
    expect(codePoints('日本語')).toBe(3);
    expect(codePoints('')).toBe(0);
  });

  it('can be cut after so many of them without splitting a pair', () => {
    expect(dropCodePoints('a😀bc', 2)).toBe('bc');
    expect(dropCodePoints('abc', 0)).toBe('abc');
    expect(dropCodePoints('abc', 9)).toBe('');
  });
});

describe('growing text', () => {
  it('is added to when a piece begins where the text ends', () => {
    const item = agentMessage('a', 'Hello ', { status: 'in_progress' });
    const outcome = applyDelta(item, 6, piece('a', 'text', 6, 'world'));
    expect(outcome.kind).toBe('applied');
    if (outcome.kind !== 'applied') return;
    expect(textOf(outcome.item)).toBe('Hello world');
    expect(outcome.applied).toBe(11);
  });

  it('is left as it is by a piece that came twice', () => {
    const item = agentMessage('a', 'Hello world', { status: 'in_progress' });
    expect(applyDelta(item, 11, piece('a', 'text', 6, 'world')).kind).toBe('behind');
    expect(applyDelta(item, 11, piece('a', 'text', 0, 'Hello ')).kind).toBe('behind');
  });

  it('keeps only the new end of a piece that overlaps what is held', () => {
    // The row it followed was ahead of the piece: "Hello wor" is held, the piece says "world" from 6.
    const item = agentMessage('a', 'Hello wor', { status: 'in_progress' });
    const outcome = applyDelta(item, 9, piece('a', 'text', 6, 'world'));
    expect(outcome.kind).toBe('applied');
    if (outcome.kind !== 'applied') return;
    expect(textOf(outcome.item)).toBe('Hello world');
    expect(outcome.applied).toBe(11);
  });

  it('is told apart by code points where a character takes two units', () => {
    const item = agentMessage('a', 'a😀', { status: 'in_progress' });
    const outcome = applyDelta(item, 2, piece('a', 'text', 1, '😀b'));
    expect(outcome.kind).toBe('applied');
    if (outcome.kind !== 'applied') return;
    expect(textOf(outcome.item)).toBe('a😀b');
    expect(outcome.applied).toBe(3);
  });

  it('is a gap when a piece begins beyond what is held', () => {
    const item = agentMessage('a', 'Hello', { status: 'in_progress' });
    expect(applyDelta(item, 5, piece('a', 'text', 9, 'x')).kind).toBe('gap');
  });

  it('is not the business of an item that does not grow by that field', () => {
    expect(applyDelta(agentMessage('a', 'x'), 1, piece('a', 'summary', 1, 'y')).kind).toBe('foreign');
    expect(applyDelta(reasoning('r', 'x'), 1, piece('r', 'text', 1, 'y')).kind).toBe('foreign');
    expect(applyDelta(reasoning('r', 'x'), 1, piece('r', 'output', 1, 'y')).kind).toBe('foreign');
  });

  it('grows a reasoning summary in the same way', () => {
    const outcome = applyDelta(reasoning('r', 'Look at', { status: 'in_progress' }), 7, piece('r', 'summary', 7, ' the handler'));
    expect(outcome.kind).toBe('applied');
    if (outcome.kind === 'applied' && outcome.item.kind === 'reasoning') expect(outcome.item.payload.summary).toBe('Look at the handler');
  });
});

describe('the output of a command', () => {
  it('grows by bytes, with the total reaching the end of the last piece', () => {
    const item = command('c', { command: 'npm test', output: 'one\n', exit: null, status: 'in_progress' });
    const outcome = applyDelta(item, 4, piece('c', 'output', 4, 'two\n', 4));
    expect(outcome.kind).toBe('applied');
    if (outcome.kind !== 'applied' || outcome.item.kind !== 'tool_call' || outcome.item.payload.tool.kind !== 'command') return;
    expect(outcome.item.payload.tool.output.inline).toBe('one\ntwo\n');
    expect(outcome.item.payload.tool.output.total_len).toBe(8);
    expect(outcome.applied).toBe(8);
  });

  it('keeps only its end when it grows long, and says that something was cut', () => {
    const big = 'x'.repeat(OUTPUT_TAIL_CHARS);
    const item = command('c', { command: 'yes', output: big, exit: null, status: 'in_progress' });
    const outcome = applyDelta(item, OUTPUT_TAIL_CHARS, piece('c', 'output', OUTPUT_TAIL_CHARS, 'END', 3));
    expect(outcome.kind).toBe('applied');
    if (outcome.kind !== 'applied' || outcome.item.kind !== 'tool_call' || outcome.item.payload.tool.kind !== 'command') return;
    const output = outcome.item.payload.tool.output;
    expect(output.inline.length).toBe(OUTPUT_TAIL_CHARS);
    expect(output.inline.endsWith('END')).toBe(true);
    expect(output.truncated).toBe(true);
    expect(output.total_len).toBe(OUTPUT_TAIL_CHARS + 3);
  });

  it('is not guessed at when a piece of bytes overlaps what is held', () => {
    const item = command('c', { command: 'x', output: 'abcdef', exit: null, status: 'in_progress' });
    expect(applyDelta(item, 6, piece('c', 'output', 3, 'defghi', 6)).kind).toBe('behind');
  });
});

describe('pieces that follow one another', () => {
  it('are one piece, for the same item and field', () => {
    const merged = mergeDeltas([piece('a', 'text', 0, 'He'), piece('a', 'text', 2, 'llo'), piece('a', 'text', 5, ' world')]);
    expect(merged).toEqual([piece('a', 'text', 0, 'Hello world')]);
  });

  it('stay apart when something is missing between them or they are for others', () => {
    const merged = mergeDeltas([piece('a', 'text', 0, 'He'), piece('a', 'text', 5, 'lo'), piece('b', 'text', 7, 'x'), piece('b', 'summary', 8, 'y')]);
    expect(merged).toHaveLength(4);
  });
});

describe('a row that arrives for an item that is being written', () => {
  it('does not take text away when it was written a moment before the last piece', () => {
    const held = agentMessage('a', 'Hello world', { status: 'in_progress' });
    const row = agentMessage('a', 'Hello', { status: 'in_progress' });
    const joined = joinRow(held, row, appliedOf(held));
    expect(textOf(joined.item)).toBe('Hello world');
    expect(joined.applied.text).toBe(11);
  });

  it('replaces the text when it is at least as long', () => {
    const held = agentMessage('a', 'Hello', { status: 'in_progress' });
    const row = agentMessage('a', 'Hello world', { status: 'in_progress' });
    const joined = joinRow(held, row, appliedOf(held));
    expect(textOf(joined.item)).toBe('Hello world');
  });

  it('is the last word when the item is no longer in progress', () => {
    const held = agentMessage('a', 'Hello world, and more', { status: 'in_progress' });
    const row = agentMessage('a', 'Hello world.', { status: 'completed' });
    const joined = joinRow(held, row, appliedOf(held));
    expect(textOf(joined.item)).toBe('Hello world.');
    expect(joined.applied.text).toBe(12);
  });
});
