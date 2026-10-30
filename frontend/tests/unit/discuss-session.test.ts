/** An unconsumed quote is one reader's unsent intent, never the next reader's draft. */
import { afterEach, expect, it, vi } from 'vitest';
import type { SessionEvent } from '@/api/client';
import type * as Client from '@/api/client';
import { discussInChat, takeQuotes, useDiscuss } from '@/features/chat/discuss';
import { threadWriter } from '@/realtime/stores/thread';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

const session = vi.hoisted(() => ({ listeners: [] as ((event: SessionEvent) => void)[], reveal: vi.fn() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, client: { ...actual.client, onSession: (callback: (event: SessionEvent) => void) => { session.listeners.push(callback); return () => undefined; } } };
});
vi.mock('@/features/workbench/openView', () => ({ revealView: session.reveal }));
afterEach(() => { threadWriter.evict(ACME.mainThread); useDiscuss.setState({ quotes: new Map() }); session.reveal.mockClear(); });
it('consumes a delayed same-reader quote once and retires another pending quote on sign-out', () => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.mainThread);
  if (person === undefined || row === undefined) throw new Error('A native thread is required');
  const thread = world.threads.thread(person, row);
  threadWriter.reset(thread.id, { thread }, false);
  expect(discussInChat(thread.id, ['selected-item'])).toBe(true);
  expect(takeQuotes(thread.chat_id)).toEqual([{ threadId: thread.id, itemId: 'selected-item' }]);
  expect(takeQuotes(thread.chat_id)).toEqual([]);
  discussInChat(thread.id, ['private-intent']);
  for (const listener of session.listeners) listener({ type: 'signed_out' });
  expect(takeQuotes(thread.chat_id)).toEqual([]);
});
