// @vitest-environment node
/** A side thread opens through its own conversation while retaining its parent's current admission. */
import { expect, it, vi } from 'vitest';
import type { Item, Thread } from '@/generated/api-types';
import { ACME, id } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';
import { Refusal, World } from '../doubles/world';

function sideIn(world: World) {
  const person = world.people.get('ada')!;
  const answer = world.change.handle(person, 'POST', `/threads/${ACME.mainThread}/side`, new URLSearchParams(), {
    client_id: id(0x99, 1), content: [{ type: 'text', text: 'Which tests cover the redirect?' }],
  });
  if (answer === null) throw new Error('Side thread was not created');
  return (answer.body as { thread: Thread }).thread.id;
}

it('opens the side conversation and its metadata without borrowing the parent draft or outline', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  world.threads.handle(person, 'PUT', `/threads/${ACME.mainThread}/draft`, new URLSearchParams(), { content: [{ type: 'text', text: 'Keep these parent words' }] });
  const side = sideIn(world);
  const opening = await threadOpening(world, person, side, 200);
  expect(opening.sync.thread.id).toBe(side);
  expect(opening.sync.thread.side_of?.id).toBe(ACME.mainThread);
  expect(opening.sync.thread.current_segment_id).toBe(opening.sync.segments.at(-1)?.id);
  expect(opening.sync.items.some((item) => item.kind === 'user_message' && item.payload.parts.some((part) => part.type === 'text' && part.text === 'Which tests cover the redirect?'))).toBe(true);
  expect(opening.draft).toBeNull();
  expect(opening.parameters).not.toBeNull();
  expect(opening.outline.entries.some((entry) => entry.title === 'Which tests cover the redirect?')).toBe(true);
  expect(opening.outline.entries.every((entry) => opening.sync.items.some((item) => item.id === entry.item_id))).toBe(true);
});

it('checks the parent view before opening and projects each viewer’s actual access', async () => {
  const world = new World();
  const side = sideIn(world);
  const viewer = world.people.get('vera')!;
  const opening = await threadOpening(world, viewer, side, 200);
  expect(opening.sync.thread.access.permissions).not.toContain('thread.interact');
  expect(opening.sync.thread.input_access.allowed).toBe(false);
  world.places.denied.set('vera', new Set(['thread.view']));
  await expect(threadOpening(world, viewer, side, 200)).rejects.toBeInstanceOf(Refusal);
});

it('keeps live side rows specific to their viewer and closes the feed after parent access is revoked', () => {
  const world = new World();
  const side = sideIn(world);
  const viewer = world.people.get('vera')!;
  const frames: unknown[] = [];
  const subscriber = { send: (frame: unknown) => { frames.push(frame); }, close: vi.fn() };
  world.change.frame(subscriber, viewer, { op: 'sub', id: 1, feed: `thread:${side}` });
  const send = (number: number) => world.change.handle(world.people.get('ada')!, 'POST', `/threads/${side}/inputs`, new URLSearchParams(), {
    client_id: id(0x99, number), content: [{ type: 'text', text: `Side input ${number}` }],
  });
  send(2);
  const sync = frames.find((frame) => typeof frame === 'object' && frame !== null && 'op' in frame && frame.op === 'sync');
  expect(sync).toMatchObject({ data: { thread: { input_access: { allowed: false } } } });
  frames.length = 0;
  world.places.denied.set('vera', new Set(['thread.view']));
  send(3);
  expect(frames).toEqual([{ op: 'closed', id: 1, feed: `thread:${side}`, reason: 'forbidden' }]);
});

it('replays the answer produced between an HTTP opening and its live subscription', async () => {
  vi.useFakeTimers();
  try {
    const world = new World();
    const person = world.people.get('ada')!;
    const side = sideIn(world);
    const opening = await threadOpening(world, person, side, 200);
    await vi.runAllTimersAsync();
    const frames: unknown[] = [];
    world.change.frame({ send: (frame) => { frames.push(frame); }, close: vi.fn() }, person, { op: 'sub', id: 1, feed: `thread:${side}`, since: opening.sync.seq });
    const replay = frames.find((frame) => typeof frame === 'object' && frame !== null && 'op' in frame && frame.op === 'sync');
    expect(replay).toMatchObject({ prev: opening.sync.seq });
    expect((replay as { data: { items: Item[] } }).data.items.some((item) => item.kind === 'agent_message')).toBe(true);
  } finally { vi.useRealTimers(); }
});
