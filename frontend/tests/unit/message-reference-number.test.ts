/** Event sequences and loaded page indexes never stand in for the shared reader's message number. */
import { expect, it } from 'vitest';
import { messageReferenceNumber, messageReferenceText } from '@/features/chat/messageReference';
import { message } from '../doubles/chat-rows';

it('requires the server-provided ordinal even when an event sequence is present', () => {
  expect(messageReferenceNumber(message(20))).toBeNull();
  const row = { ...message(20), reference_number: 3 };
  expect(messageReferenceNumber(row)).toBe(3);
});

it.each(['Project chat', '项目讨论'])('copies the actual parent and the native ordinal for %s', (name) => {
  const row = { ...message(20), chat_id: '00000000-0000-4000-8000-000000000071', reference_number: 3 };
  expect(messageReferenceText(row, name)).toBe(`[${name}](id:${row.chat_id}#3)`);
  expect(messageReferenceText(message(20), name)).toBeNull();
});

it.each([0, -1, 1.5, Number.NaN, Number.MAX_SAFE_INTEGER + 1, '3', null])('rejects an invalid reference number %s', (reference_number) => {
  const row = { ...message(20), reference_number };
  expect(messageReferenceNumber(row)).toBeNull();
});
