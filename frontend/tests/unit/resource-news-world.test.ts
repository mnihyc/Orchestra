/** A native fake subscription reports later changes, not an invented initial change to every row. */
import { expect, it } from 'vitest';
import type { Provider } from '../../src/generated/api-types.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { World } from '../doubles/world.ts';

it('keeps initial resources quiet and publishes an actual later rename to the same subscriber', async () => {
  const world = new World();
  world.requestFeeds.control('/fixtures');
  const person = world.people.get('ada');
  if (person === undefined) throw new Error('Missing fixture reader');
  const frames: unknown[] = [];
  const subscriber = { send: (frame: unknown) => { frames.push(frame); }, close: () => undefined };
  expect(world.users.frame(subscriber, person, { op: 'sub', id: 1, feed: 'user' })).toBe(true);
  frames.length = 0;
  await world.requestFeeds.changed();
  expect(frames).toEqual([]);
  const path = `/providers/${RUNS_ON.openAiKey}`;
  const before = world.resources.handle(person, 'GET', path, new URLSearchParams(), {})?.body as Provider;
  const changed = world.resources.handle(person, 'PATCH', path, new URLSearchParams(), { name: 'Renamed fixture provider', expected_revision: before.revision });
  expect(changed?.status).toBe(200);
  await world.requestFeeds.changed();
  const renamedRow: unknown = expect.objectContaining({ name: 'Renamed fixture provider' });
  const resourceEvent: unknown = expect.objectContaining({ what: 'resources', kind: 'provider', id: RUNS_ON.openAiKey, row: renamedRow });
  expect(frames).toEqual([expect.objectContaining({ op: 'event', feed: 'user', data: resourceEvent })]);
  frames.length = 0;
  await world.requestFeeds.changed();
  expect(frames).toEqual([]);
});
