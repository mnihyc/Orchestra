/** An unconsumed quote is one reader's unsent intent, never the next reader's draft. */
import { afterEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { discussInChat, mentionInChat, takeMentions, takeQuotes, useDiscuss } from '@/features/chat/discuss';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';
import { ADA, BEA } from './general-access-fixtures';

const session = vi.hoisted(() => ({ reveal: vi.fn() }));
vi.mock('@/features/workbench/openView', () => ({ revealView: session.reveal }));
const initial = client.session.getState();
const reader = () => client.session.getState().readerVersion ?? 0;
const signOut = () => client.session.setState({ status: 'signed_out', userId: null, readerVersion: reader() + 1 });
afterEach(() => {
  client.session.setState({ ...initial, readerVersion: reader() + 1 });
  threadWriter.evict(ACME.mainThread);
  useDiscuss.setState({ quotes: new Map(), mentions: new Map() });
  session.reveal.mockClear();
});
it('consumes a delayed same-reader quote once and retires another pending quote on sign-out', () => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A native thread is required');
  const thread = world.threads.thread(person, row);
  threadWriter.reset(thread.id, { thread }, false);
  expect(discussInChat(thread.id, ['selected-item'])).toBe(true);
  expect(takeQuotes(thread.chat_id)).toEqual([{ threadId: thread.id, itemId: 'selected-item' }]);
  expect(takeQuotes(thread.chat_id)).toEqual([]);
  discussInChat(thread.id, ['private-intent']);
  signOut();
  expect(takeQuotes(thread.chat_id)).toEqual([]);
});

it('asks an owner in the existing thread chat without sending and retires an unsent mention on sign-out', () => {
  const owner = ADA;
  mentionInChat('thread', 'thread-chat', owner);
  expect(session.reveal).toHaveBeenLastCalledWith('chat', { node: 'thread' });
  expect(takeMentions('thread-chat')).toEqual([{ name: owner.display_name, target: { kind: 'user', id: owner.id } }]);
  expect(takeMentions('thread-chat')).toEqual([]);
  mentionInChat('thread', 'thread-chat', owner);
  signOut();
  expect(takeMentions('thread-chat')).toEqual([]);
});

it('retires an unsent owner mention when a direct sign-in replaces its reader', () => {
  client.session.setState({ status: 'signed_in', userId: ADA.id, readerVersion: reader() + 1 });
  mentionInChat('thread', 'thread-chat', ADA);
  client.session.setState({ status: 'signed_in', userId: BEA.id, readerVersion: reader() + 1 });
  expect(takeMentions('thread-chat')).toEqual([]);
});

it('keeps an unsent owner mention when only the same readers session expiry changes', () => {
  client.session.setState({ status: 'signed_in', userId: ADA.id, readerVersion: reader() + 1 });
  mentionInChat('thread', 'thread-chat', ADA);
  client.session.setState({ accessEndsAt: Date.now() + 60_000 });
  expect(takeMentions('thread-chat')).toEqual([{ name: ADA.display_name, target: { kind: 'user', id: ADA.id } }]);
});
