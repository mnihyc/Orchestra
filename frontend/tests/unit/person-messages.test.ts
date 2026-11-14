/** The people's messages are what the keys, the Messages list and the sticky prompt move between; the Outline lists them without what the agent says between its tools. */
import { describe, expect, it } from 'vitest';
import { outlineRows } from '@/features/inspector/outlineRows';
import { isPersonMessage, lineCountsByTurn, messageAt, messageBeside, personMessages } from '@/features/thread/messages';
import { roomFor } from '@/features/thread/replyRoom';
import type { Item, OutlineEntry } from '@/generated/api-types';
import { me, userRef } from '../doubles/fixtures';
import { agentMessage, change, fileChange, userMessage } from '../doubles/thread-rows';

const ADA = userRef(me(1, 'ada', 'Ada'));
const VERA = userRef(me(2, 'vera', 'Vera'));
const at = (item: Item, seq: number): Item => ({ ...item, seq_created: seq, seq_updated: seq });
const held = (...items: Item[]) => ({ items: new Map(items.map((item) => [item.id, item])), order: items.map((item) => item.id) });

const conversation = [
  at(userMessage('first', 'Work in a new directory\nand explain each step', { turn: 't1', author: ADA }), 2),
  at(agentMessage('says-1', 'I will start.', { turn: 't1', phase: 'commentary' }), 3),
  at(fileChange('edit-1', { turn: 't1', changes: [change('a.py', null, { added: 10, removed: 3 })] }), 4),
  at(agentMessage('answer-1', 'Done.', { turn: 't1' }), 5),
  at(userMessage('second', 'Now the tests', { turn: 't2', author: VERA }), 10),
  at(userMessage('told', 'A line of the system', { turn: 't2', author: ADA, source: 'system' }), 11),
  at(agentMessage('answer-2', 'Tested.', { turn: 't2' }), 12),
];

describe('the people’s messages', () => {
  it('are the messages a person said to the agent, and not what the system or an announcement said', () => {
    const { items, order } = held(...conversation);
    expect(personMessages(items, order).map((message) => message.id)).toEqual(['first', 'second']);
    expect(isPersonMessage(conversation[5]!)).toBe(false);
  });

  it('carry who said them, the first line and the whole text, and the lines their turn changed', () => {
    const { items, order } = held(...conversation);
    const [first, second] = personMessages(items, order);
    expect(first).toMatchObject({ author: ADA, line: 'Work in a new directory', text: 'Work in a new directory\nand explain each step', turnId: 't1', added: 10, removed: 3 });
    expect(second).toMatchObject({ author: VERA, line: 'Now the tests', added: 0, removed: 0 });
    expect(lineCountsByTurn(items, order).get('t1')).toEqual({ added: 10, removed: 3 });
  });

  it('are found from a place in the conversation: the one the reader is in the middle of, and the one before and after', () => {
    const { items, order } = held(...conversation);
    const messages = personMessages(items, order);
    expect(messageAt(messages, 4)?.id).toBe('first');
    expect(messageAt(messages, 10)?.id).toBe('second');
    expect(messageAt(messages, 1)).toBeNull();
    expect(messageBeside(messages, 4, -1)?.id).toBe('first');
    expect(messageBeside(messages, 2, -1)).toBeNull();
    expect(messageBeside(messages, 4, 1)?.id).toBe('second');
    // From a message itself, the one before is the one before it.
    expect(messageBeside(messages, 10, -1)?.id).toBe('first');
    expect(messageBeside(messages, 10, 1)).toBeNull();
  });
});

describe('the room under a sent message', () => {
  it('is what the view lacks of the conversation from that place to its end, and nothing once the reply fills the view', () => {
    // A view 600 high, the message 1,000 down, the conversation ending 100 below it: 500 more are needed.
    expect(roomFor(1_000, 600, 1_100)).toBe(500);
    expect(roomFor(1_000, 600, 1_600)).toBe(0);
    expect(roomFor(1_000, 600, 2_000)).toBe(0);
  });
});

describe('the Outline', () => {
  const entry = (kind: OutlineEntry['kind'], id: string, seq: number, more: Partial<OutlineEntry> = {}): OutlineEntry => ({ item_id: id, seq, at: '2026-10-15T00:00:00Z', title: id, kind, ...more });

  it('lists a turn’s answer and not the sentences between its tools', () => {
    const stored = [entry('turn', 'm1', 2, { turn_ordinal: 1, title: '' }), entry('user_message', 'm1', 2), entry('agent_message', 'says-1', 3), entry('agent_message', 'says-2', 4), entry('agent_message', 'answer', 6),
      entry('turn', 'm2', 10, { turn_ordinal: 2, title: '' }), entry('user_message', 'm2', 10), entry('agent_message', 'answer-2', 12)];
    const rows = outlineRows(stored, { order: [], items: new Map(), turns: new Map() });
    expect(rows.map((row) => `${row.kind}:${row.item_id}`)).toEqual(['turn:m1', 'user_message:m1', 'agent_message:answer', 'turn:m2', 'user_message:m2', 'agent_message:answer-2']);
  });

  it('keeps what a person steered into a turn as the end of one stretch and the start of the next', () => {
    const stored = [entry('turn', 'm1', 2, { turn_ordinal: 1, title: '' }), entry('user_message', 'm1', 2), entry('agent_message', 'says', 3), entry('user_message', 'steer', 4), entry('agent_message', 'answer', 6)];
    expect(outlineRows(stored, { order: [], items: new Map(), turns: new Map() }).map((row) => row.item_id)).toEqual(['m1', 'm1', 'says', 'steer', 'answer']);
  });
});
