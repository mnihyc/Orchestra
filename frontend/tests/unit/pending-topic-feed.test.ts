/** The browser double uses its native topic snapshot clock instead of refusing a declared live source. */
import { expect, it, vi } from 'vitest';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

it('publishes the same authorized topic cut used by the opening and stops when unsubscribed', async () => {
  const world = new World(), ada = world.people.get('ada');
  if (ada === undefined) throw new Error('Expected the native reader');
  const send = vi.fn(), subscriber = { send, close: vi.fn() }, feed = `topic:${ACME.fixLogin}`;
  const native = await world.requestFeeds.read(ada, `/topics/${ACME.fixLogin}/sync`);
  expect(world.requestFeeds.frame(subscriber, ada, { op: 'sub', id: 91, feed, since: null })).toBe(true);
  await world.requestFeeds.changed();
  expect(send).toHaveBeenCalledWith(expect.objectContaining({ op: 'sync', feed, data: native.body }));
  expect(send).not.toHaveBeenCalledWith(expect.objectContaining({ op: 'error' }));
  expect(world.requestFeeds.frame(subscriber, ada, { op: 'unsub', id: 91 })).toBe(true);
  send.mockClear(); await world.requestFeeds.changed(); expect(send).not.toHaveBeenCalled();
});

it('refuses the topic source for an anonymous subscriber', () => {
  const world = new World(), send = vi.fn();
  expect(world.requestFeeds.frame({ send, close: vi.fn() }, undefined, { op: 'sub', id: 92, feed: `topic:${ACME.fixLogin}` })).toBe(true);
  expect(send).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ op: 'error', code: 'forbidden' }));
});
