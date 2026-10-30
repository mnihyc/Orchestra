// @vitest-environment node
/** Provider limits come from the same admitted ledger that accepts their changes. */
import { expect, it, vi } from 'vitest';
import type { UsageCap } from '@/generated/api-types';
import { RUNS_ON } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

async function read(world: World, username = 'ada') {
  const person = world.people.get(username)!;
  for (const { part } of world.parts) {
    const answer = await part.handle?.(person, 'GET', '/usage/caps', new URLSearchParams({ provider_id: RUNS_ON.openAiKey }), {}) ?? null;
    if (answer !== null) return answer;
  }
  throw new Error('No provider-cap reader');
}

it('has no invented provider caps before anybody creates one', async () => {
  expect((await read(new World())).body).toEqual([]);
});

it('reads the provider cap accepted by the native usage writer', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const cap = world.usage.handle(person, 'POST', '/usage/caps', new URLSearchParams(), {
    provider_id: RUNS_ON.openAiKey, subject: { kind: 'everyone' }, split: 'shared', meter: 'tokens', amount: '5000000', period: 'day',
  })!.body as UsageCap;
  expect((await read(world)).body).toMatchObject([{ cap: { id: cap.id, amount: '5000000', provider: { id: RUNS_ON.openAiKey } } }]);
});

it('refuses the provider limits to somebody who cannot view that provider', async () => {
  const world = new World();
  const native = world.resources.handle.bind(world.resources);
  vi.spyOn(world.resources, 'handle').mockImplementation((...args) => {
    if (args[1] === 'GET' && args[2] === `/providers/${RUNS_ON.openAiKey}`) throw new Refusal(403, 'authz.denied', 'this provider is private');
    return native(...args);
  });
  await expect(read(world, 'zoe')).rejects.toThrow(Refusal);
});
