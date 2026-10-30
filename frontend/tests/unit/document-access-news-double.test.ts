// @vitest-environment node
/** Document permission changes reach current readers and close readers who lose View before another snapshot. */
import { expect, it, vi } from 'vitest';
import { id } from '../doubles/fixtures';
import { Refusal, type Subscriber, World } from '../doubles/world';

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
const sync = (frames: unknown[]) => frames.find((frame) => record(frame) && frame.op === 'sync');

it('publishes Private to the owner and closes a former reader, then publishes the reset to the owner', async () => {
  const world = new World();
  const owner = world.people.get('ada')!, reader = world.people.get('dora')!;
  const document = id(7, 1);
  const own: unknown[] = [], other: unknown[] = [];
  const closeOwner = vi.fn(), closeReader = vi.fn();
  const first: Subscriber = { send: (frame) => { own.push(frame); }, close: closeOwner };
  const second: Subscriber = { send: (frame) => { other.push(frame); }, close: closeReader };
  world.documents.frame(first, owner, { op: 'sub', id: 1, feed: `doc:${document}` });
  world.documents.frame(second, reader, { op: 'sub', id: 2, feed: `doc:${document}` });
  expect(other.some((frame) => record(frame) && frame.op === 'subscribed')).toBe(true);
  own.length = 0; other.length = 0;
  world.access.handle(owner, 'PATCH', `/documents/${document}/permissions`, new URLSearchParams(), { whole: 'private' });
  expect(sync(own)).toMatchObject({ data: { document: { id: document, page: 'restricted' } } });
  expect(other).toContainEqual({ op: 'closed', id: 2, feed: `doc:${document}`, reason: 'forbidden' });
  expect(other.some((frame) => record(frame) && frame.op === 'sync')).toBe(false);
  await expect(world.requestFeeds.read(reader, `/documents/${document}/sync`)).rejects.toThrow(Refusal);
  own.length = 0; other.length = 0;
  world.access.handle(owner, 'PATCH', `/documents/${document}/permissions`, new URLSearchParams(), { whole: 'reset' });
  expect(sync(own)).toMatchObject({ data: { document: { id: document, page: 'follows' } } });
  expect(other).toEqual([]);
  expect(closeOwner).not.toHaveBeenCalled(); expect(closeReader).not.toHaveBeenCalled();
  world.documents.gone(first); world.documents.gone(second);
});
