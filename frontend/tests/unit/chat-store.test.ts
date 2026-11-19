/** What the page keeps of a chat: rows replace only what is older, messages stand in the order they were written, a window of the past is not broken by what is new, who is typing, and what is unread. */
import { beforeEach, describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import type { Problem } from '@/generated/api-types';
import {
  chatWriter,
  EMPTY_ENTRY,
  isUnread,
  mergeState,
  statesWriter,
  useChatStates,
  newestHeld,
  reduceAround,
  reduceFailed,
  reduceNewer,
  reduceOlder,
  reduceReset,
  reduceRows,
  TYPING_SHOWN_MS,
  typingNow,
  useChatStore,
} from '@/realtime/stores/chat';

import { ADA, chat, DORA, GRACE, message, state } from '../doubles/chat-rows';

const ids = (entry: { order: readonly string[]; messages: ReadonlyMap<string, { seq_created: number }> }) =>
  entry.order.map((id) => entry.messages.get(id)?.seq_created);

describe('the rows of a chat', () => {
  it('stand in the order they were written, whatever order they arrive in', () => {
    const opened = reduceReset(EMPTY_ENTRY, chat({ seq: 3 }), [message(2), message(3)], false, 3);
    const entry = reduceRows(opened, { messages: [message(1)] }, 4);
    expect(ids(entry)).toEqual([1, 2, 3]);
    expect(entry.lastSeq).toBe(4);
  });

  it('are replaced only by a row that changed later: an old row arriving late changes nothing', () => {
    const opened = reduceReset(EMPTY_ENTRY, chat(), [message(1), message(2)], false, 2);
    const edited = reduceRows(
      opened,
      { messages: [message(1, { parts: [{ type: 'text', text: 'edited' }], edited_at: '2026-10-01T09:05:00.000Z', seq_updated: 5 })] },
      5,
    );
    expect(edited.messages.get(message(1).id)?.parts).toEqual([{ type: 'text', text: 'edited' }]);
    const late = reduceRows(edited, { messages: [message(1)] }, 5);
    // The same object: nothing was written.
    expect(late.messages.get(message(1).id)?.seq_updated).toBe(5);
    expect(late.messages.get(message(1).id)?.parts).toEqual([{ type: 'text', text: 'edited' }]);
  });

  it('take an edit, a reaction, a pin and a deletion each as the message\'s own row, without a second copy of the message', () => {
    let entry = reduceReset(EMPTY_ENTRY, chat(), [message(1)], false, 1);
    entry = reduceRows(entry, { messages: [message(1, { reactions: [{ emoji: '👍', users: [GRACE.id] }], seq_updated: 2 })] }, 2);
    entry = reduceRows(entry, { messages: [message(1, { pinned: { at: '2026-10-01T09:06:00.000Z', by: GRACE }, seq_updated: 3 })] }, 3);
    entry = reduceRows(entry, { messages: [message(1, { deleted: true, parts: [], seq_updated: 4 })] }, 4);
    expect(entry.order).toHaveLength(1);
    expect(entry.messages.get(message(1).id)).toMatchObject({ deleted: true, parts: [] });
  });

  it('are never removed by the page: a deleted message stays as the row that says so', () => {
    const entry = reduceRows(
      reduceReset(EMPTY_ENTRY, chat(), [message(1), message(2)], false, 2),
      { messages: [message(2, { deleted: true, parts: [], seq_updated: 3 })] },
      3,
    );
    expect(entry.order).toHaveLength(2);
    expect(entry.messages.get(message(2).id)?.deleted).toBe(true);
  });

  it('replace the chat itself only by one that is not older', () => {
    const opened = reduceReset(EMPTY_ENTRY, chat({ seq: 5, name: 'New' }), [], false, 5);
    expect(reduceRows(opened, { chat: chat({ seq: 4, name: 'Old' }) }, 5).chat?.name).toBe('New');
    expect(reduceRows(opened, { chat: chat({ seq: 6, name: 'Newer' }) }, 6).chat?.name).toBe('Newer');
  });

  it('end the typing of whoever wrote a new message', () => {
    const typing = new Map([[GRACE.id, { user: GRACE, at: 1_000 }]]);
    const opened = { ...reduceReset(EMPTY_ENTRY, chat(), [message(1)], false, 1), typing };
    const entry = reduceRows(opened, { messages: [message(2, { author: GRACE })] }, 2);
    expect(entry.typing.has(GRACE.id)).toBe(false);
  });
});

/** What the API answers when a chat cannot be read: 403 for the right that is missing, 404 for what is not there. */
function answered(status: number): ApiError {
  const problem = {
    type: 'orch:error:not_found',
    code: status === 403 ? 'authz.denied' : 'not_found',
    title: 'No',
    status,
    detail: 'no',
    request_id: 'req-1',
  } as unknown as Problem;
  return new ApiError(problem, null);
}

describe('a load that failed', () => {
  it('ends the reading of a chat that was held when the server says it can no longer be read', () => {
    const held = reduceReset(EMPTY_ENTRY, chat({ kind: 'group' }), [message(1)], false, 1);
    expect(reduceFailed(held, answered(404)).closed).toBe('revoked');
    expect(reduceFailed(held, answered(403)).closed).toBe('revoked');
    // A place's chat that is not there any more is gone, not closed to the reader.
    const place = reduceReset(EMPTY_ENTRY, chat({ kind: 'node' }), [], false, 0);
    expect(reduceFailed(place, answered(404)).closed).toBe('deleted');
  });

  it('only notes the error when nothing was held, or when it was the network that failed', () => {
    expect(reduceFailed(EMPTY_ENTRY, answered(404))).toMatchObject({ closed: null });
    const held = reduceReset(EMPTY_ENTRY, chat({ kind: 'group' }), [message(1)], false, 1);
    const network = new TypeError('Failed to fetch');
    expect(reduceFailed(held, network)).toMatchObject({ closed: null, error: network });
    expect(reduceFailed(held, answered(500))).toMatchObject({ closed: null });
  });
});

describe('a window of the past', () => {
  const window = reduceAround(
    reduceReset(EMPTY_ENTRY, chat(), [message(50), message(51)], true, 51),
    { messages: [message(10), message(11), message(12)], has_older: true, has_newer: true },
  );

  it('is not broken by what is new: a message beyond it waits, and says that newer ones exist', () => {
    const entry = reduceRows(window, { messages: [message(52)] }, 52);
    expect(ids(entry)).toEqual([10, 11, 12]);
    expect(entry.hasNewer).toBe(true);
    expect(entry.lastSeq).toBe(52);
  });

  it('still takes a change to a message it holds', () => {
    const entry = reduceRows(window, { messages: [message(11, { seq_updated: 60, deleted: true, parts: [] })] }, 60);
    expect(entry.messages.get(message(11).id)?.deleted).toBe(true);
  });

  it('becomes the tail when the newer messages have been read to the end', () => {
    const entry = reduceNewer(window, { messages: [message(13), message(14)], has_newer: false });
    expect(entry.loaded).toBe('tail');
    expect(ids(entry)).toEqual([10, 11, 12, 13, 14]);
    // From then on a new message is laid in its place, as in any tail.
    expect(ids(reduceRows(entry, { messages: [message(15)] }, 15))).toEqual([10, 11, 12, 13, 14, 15]);
  });

  it('is left for the tail when the reader asks for the newest messages again', () => {
    const entry = reduceReset(window, chat(), [message(50), message(51)], true, 51);
    expect(entry.loaded).toBe('tail');
    expect(entry.hasNewer).toBe(false);
    expect(ids(entry)).toEqual([50, 51]);
  });
});

describe('older messages', () => {
  it('are laid before the ones held, and say whether still older ones exist', () => {
    const opened = reduceReset(EMPTY_ENTRY, chat(), [message(5), message(6)], true, 6);
    const entry = reduceOlder(opened, { messages: [message(3), message(4), message(5)], has_older: false });
    expect(ids(entry)).toEqual([3, 4, 5, 6]);
    expect(entry.hasOlder).toBe(false);
    expect(newestHeld(entry)).toBe(6);
  });
});

describe('who is typing', () => {
  const typing = new Map([
    [GRACE.id, { user: GRACE, at: 10_000 }],
    [DORA.id, { user: DORA, at: 12_000 }],
    [ADA.id, { user: ADA, at: 12_500 }],
  ]);

  it('is everybody but the reader whose last sign is younger than six seconds, the earliest first', () => {
    expect(typingNow(typing, 13_000, ADA.id).map((user) => user.username)).toEqual(['grace', 'dora']);
  });

  it('is gone six seconds after the last sign', () => {
    expect(typingNow(typing, 10_000 + TYPING_SHOWN_MS, ADA.id).map((user) => user.username)).toEqual(['dora']);
    expect(typingNow(typing, 12_000 + TYPING_SHOWN_MS, ADA.id)).toEqual([]);
  });
});

describe('what is unread', () => {
  it('updates scope indicators from a chat snapshot/live message even when the hierarchy is not followed', () => {
    chatWriter.clear(); statesWriter.clear();
    const id = chat().id;
    const standing = state(id, { last_read_seq: 1, effective_level: 'all' });
    chatWriter.reset(id, chat({ my: standing, last_message_seq: 2 }), [message(2)], false, 20);
    let held = useChatStates.getState();
    expect(isUnread(held.states.get(id), held.latest.get(id)?.seq ?? 0)).toBe(true);
    statesWriter.read(id, 2, standing);
    // A metadata/reaction frame cannot invent another unread message or undo the read.
    chatWriter.apply(id, { chat: chat({ my: standing, last_message_seq: 2, seq: 21 }), messages: [message(2, { seq_updated: 21 })] }, 21);
    held = useChatStates.getState();
    expect(held.latest.get(id)?.seq).toBe(2);
    expect(isUnread(held.states.get(id), 2)).toBe(false);
    chatWriter.apply(id, { messages: [message(3, { author: GRACE })] }, 22);
    held = useChatStates.getState();
    expect(isUnread(held.states.get(id), held.latest.get(id)?.seq ?? 0)).toBe(true);
    chatWriter.clear(); statesWriter.clear();
  });

  it('is something newer than the position read, in a chat that the person follows', () => {
    expect(isUnread(state('c', { last_read_seq: 4 }), 5)).toBe(true);
    expect(isUnread(state('c', { last_read_seq: 5 }), 5)).toBe(false);
  });

  it('is nothing in a chat that is not followed, and nothing where the person has no state', () => {
    expect(isUnread(state('c', { effective_level: 'off', last_read_seq: 0 }), 9)).toBe(false);
    expect(isUnread(null, 9)).toBe(false);
  });

  it('keeps a position that only moves forward when a state that is older arrives late', () => {
    const held = state('c', { last_read_seq: 8, mentions_unread: 0 });
    expect(mergeState(held, state('c', { last_read_seq: 5, mentions_unread: 2 })).last_read_seq).toBe(8);
    expect(mergeState(held, state('c', { last_read_seq: 9, mentions_unread: 1 }))).toMatchObject({ last_read_seq: 9, mentions_unread: 1 });
  });
});

describe('a message that was not sent', () => {
  beforeEach(() => chatWriter.clear());

  it('stays with its text when the chat is forgotten, and goes when it is sent', () => {
    const sent = { clientId: 'k1', parts: [{ type: 'text' as const, text: 'hello' }], replyTo: null, at: 1, state: 'failed' as const, error: null };
    chatWriter.reset('c', chat({ id: 'c' }), [message(1)], false, 1);
    chatWriter.outgoing('c', sent);
    chatWriter.evict('c');
    expect(useChatStore.getState().byChat.get('c')?.outbox).toEqual([sent]);
    expect(useChatStore.getState().byChat.get('c')?.messages.size).toBe(0);
    chatWriter.outgoingDone('c', 'k1');
    chatWriter.evict('c');
    expect(useChatStore.getState().byChat.has('c')).toBe(false);
  });

  it('is put once whatever number of times it is put with the same key', () => {
    const one = { clientId: 'k2', parts: [{ type: 'text' as const, text: 'a' }], replyTo: null, at: 1, state: 'sending' as const, error: null };
    chatWriter.outgoing('d', one);
    chatWriter.outgoing('d', { ...one, state: 'failed' });
    expect(useChatStore.getState().byChat.get('d')?.outbox).toHaveLength(1);
    expect(useChatStore.getState().byChat.get('d')?.outbox[0]?.state).toBe('failed');
  });
});
