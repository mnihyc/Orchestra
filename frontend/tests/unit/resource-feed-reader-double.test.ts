// @vitest-environment node
/** Native resource events retain the same browser principal as the ordinary HTTP read. */
import { expect, it, vi } from 'vitest';

import type { AuthResult, Provider } from '../../src/generated/api-types.ts';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { World, type Person, type Subscriber } from '../doubles/world.ts';

const providerId = RUNS_ON.teamAnthropic;
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

function login(world: World) {
  const answer = world.login({ login: 'ada', password: 'correct horse battery' });
  expect(answer.status).toBe(200);
  const auth = answer.body as AuthResult;
  return `Bearer ${auth.access_token}`;
}

function listen(world: World, person: Person, id = 7) {
  const frames: unknown[] = [];
  const subscriber: Subscriber = { send: (frame) => { frames.push(frame); }, close: vi.fn() };
  expect(world.users.frame(subscriber, person, { op: 'sub', id, feed: 'user' })).toBe(true);
  frames.length = 0;
  return { subscriber, frames };
}

function rows(frames: readonly unknown[]) {
  return frames.flatMap((frame) => record(frame) && record(frame.data) && frame.data.what === 'resources'
    && frame.data.kind === 'provider' && frame.data.id === providerId ? [frame.data] : []);
}

async function read(world: World, person: Person): Promise<Provider> {
  const answer = await world.requestFeeds.read(person, `/providers/${providerId}`);
  expect(answer.status).toBe(200);
  return answer.body as Provider;
}

function rename(world: World, person: Person, name: string) {
  const before = world.resources.handle(person, 'GET', `/providers/${providerId}`, new URLSearchParams(), {})?.body as Provider;
  const answer = world.resources.handle(person, 'PATCH', `/providers/${providerId}`, new URLSearchParams(), { name, expected_revision: before.revision });
  expect(answer?.status).toBe(200);
}

it('projects the same provider separately for ordinary and explicit-admin sessions of one person', async () => {
  const world = new World(), ordinaryToken = login(world), adminToken = login(world);
  world.setAdminMode(adminToken, { as_admin: true });
  const ordinary = world.browserPerson(ordinaryToken), admin = world.browserPerson(adminToken);
  const a = listen(world, ordinary), b = listen(world, admin);
  rename(world, admin, 'A real provider change for both readers');
  const expected = await read(world, ordinary), elevated = await read(world, admin);
  expect(expected.access.permissions).toEqual(['provider.view', 'provider.use']);
  expect(elevated.access.permissions).toContain('provider.manage');
  await world.requestFeeds.changed();
  expect(rows(a.frames)).toHaveLength(1);
  expect(rows(b.frames)).toHaveLength(1);
  expect(rows(a.frames)[0]?.row).toEqual(expected);
  expect(rows(b.frames)[0]?.row).toEqual(elevated);
  a.frames.length = 0; b.frames.length = 0;
  await world.requestFeeds.changed();
  expect(rows(a.frames)).toEqual([]);
  expect(rows(b.frames)).toEqual([]);
});

it('retires an in-flight former listener before publishing and admits a reopened reader independently', async () => {
  const world = new World(), token = login(world), person = world.browserPerson(token);
  const adminToken = login(world); world.setAdminMode(adminToken, { as_admin: true });
  const admin = world.browserPerson(adminToken);
  const old = listen(world, person);
  rename(world, admin, 'Changed before the former reader waits');
  const nativeRead = world.requestFeeds.read.bind(world.requestFeeds);
  let release: () => void = () => undefined, started: () => void = () => undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  const began = new Promise<void>((resolve) => { started = resolve; });
  vi.spyOn(world.requestFeeds, 'read').mockImplementationOnce(async (reader, path) => {
    const answer = await nativeRead(reader, path);
    started(); await pending;
    return answer;
  });
  const changing = world.requestFeeds.changed();
  await began;
  world.users.gone(old.subscriber);
  const current = listen(world, world.browserPerson(token), 9);
  release(); await changing;
  expect(rows(old.frames)).toEqual([]);
  expect(rows(current.frames)).toEqual([]);
  rename(world, admin, 'Changed after the current reader joined');
  await world.requestFeeds.changed();
  expect(rows(current.frames)).toHaveLength(1);
  expect(rows(current.frames)[0]?.row).toEqual(await read(world, person));
});

it('withdraws revoked Use from the ordinary roster while retaining only the independently authorized admin projection', async () => {
  const world = new World(), ordinaryToken = login(world), adminToken = login(world);
  world.setAdminMode(adminToken, { as_admin: true });
  const ordinary = world.browserPerson(ordinaryToken), admin = world.browserPerson(adminToken);
  const a = listen(world, ordinary), b = listen(world, admin);
  await world.requestFeeds.changed();
  a.frames.length = 0; b.frames.length = 0;
  const provider = world.resources.providers.find((row) => row.id === providerId);
  if (provider === undefined) throw new Error('The standing provider fixture is missing');
  provider.uses = provider.uses.filter((username) => username !== 'ada');
  expect((await read(world, ordinary)).access.permissions).toEqual(['provider.view']);
  const elevated = await read(world, admin);
  expect(elevated.access.permissions).toContain('provider.manage');
  await world.requestFeeds.changed();
  expect(rows(a.frames)).toEqual([{ what: 'resources', kind: 'provider', id: providerId, deleted: true }]);
  expect(rows(b.frames)).toHaveLength(1);
  expect(rows(b.frames)[0]?.row).toEqual(elevated);
  world.users.gone(b.subscriber);
  world.setAdminMode(adminToken, { as_admin: false });
  const lowered = listen(world, world.browserPerson(adminToken));
  await world.requestFeeds.changed();
  expect(rows(lowered.frames)).toEqual([]);
  expect((await read(world, world.browserPerson(adminToken))).access.permissions).toEqual(['provider.view']);
});
