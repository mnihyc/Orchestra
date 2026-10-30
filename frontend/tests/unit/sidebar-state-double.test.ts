// @vitest-environment node
/** Sidebar choices use the canonical personal-state writer and reach every connection of their owner. */
import { expect, it, vi } from 'vitest';
import { World, type Person } from '../doubles/world';

it('persists sidebar choices and tells both owner connections without telling another person', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const first: unknown[] = [], second: unknown[] = [], other: unknown[] = [];
  for (const [reader, frames] of [[person, first], [person, second], [world.people.get('vera')!, other]] as [Person, unknown[]][]) {
    world.users.frame({ send: (frame) => { frames.push(frame); }, close: vi.fn() }, reader, { op: 'sub', id: 1, feed: 'user' });
    frames.length = 0;
  }
  const value = { folded: ['recent', 'projects'], show: { threads: true, documents: true, chats: false }, hide_done: false };
  let status: number | undefined;
  for (const { part } of world.parts) {
    const answer = await part.handle?.(person, 'PUT', '/me/state/sidebar', new URLSearchParams(), value);
    if (answer !== null && answer !== undefined) { status = answer.status; break; }
  }
  expect(status).toBe(204);
  expect(world.settings.handle(person, 'GET', '/me/state', new URLSearchParams(), {})?.body).toMatchObject({ sidebar: value });
  const event = { op: 'event', id: 1, feed: 'user', data: { what: 'state', key: 'sidebar', value } };
  expect(first).toEqual([event]);
  expect(second).toEqual([event]);
  expect(other).toEqual([]);
});
