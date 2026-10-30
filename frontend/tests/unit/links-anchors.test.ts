/** A click on a mention goes to the part of its source where the mention stands. */
import { expect, it } from 'vitest';

import { mentionPath, sourcePath, whereOf } from '@/features/links/anchors';

const document = { kind: 'document', id: 'doc' } as const;
const chat = { kind: 'chat', id: 'chat' } as const;
const thread = { kind: 'thread', id: 'thread' } as const;

it('opens a document at the block, or at the block a comment is about', () => {
  expect(mentionPath(document, { type: 'block', block_id: 'b 1' })).toBe('/doc/doc?block=b%201');
  expect(mentionPath(document, { type: 'comment', comment_id: 'c', discussion_id: 'd', block_id: 'b' })).toBe('/doc/doc?block=b');
  expect(mentionPath(document, { type: 'comment', comment_id: 'c', discussion_id: 'd', block_id: null })).toBe('/doc/doc');
});

it('opens a chat at the message and a thread at the turn', () => {
  expect(mentionPath(chat, { type: 'message', message_id: 'm' })).toBe('/chat/chat?message=m');
  expect(mentionPath(thread, { type: 'turn', item_id: 'i', turn: 4 })).toBe('/thread/thread?turn=4');
  expect(mentionPath(thread, { type: 'turn', item_id: 'i', turn: null })).toBe('/thread/thread');
  expect(sourcePath(thread)).toBe('/thread/thread');
});

it('says where a mention stands, with the number of a turn', () => {
  expect(whereOf({ type: 'turn', item_id: 'i', turn: 4 })).toEqual({ key: 'links.where.turn_number', number: 4 });
  expect(whereOf({ type: 'comment', comment_id: 'c', discussion_id: 'd', block_id: null }).key).toBe('links.where.comment');
});
