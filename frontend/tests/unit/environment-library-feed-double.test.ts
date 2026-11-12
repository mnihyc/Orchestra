// @vitest-environment node
/** HTTP and native Environment openings reuse the same current-reader Library projection. */
import { expect, it, vi } from 'vitest';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { World, Refusal, type Person, type Subscriber } from '../doubles/world.ts';

const id = RUNS_ON.workspace;
const target = { object: { kind: 'environment', id }, version_id: null };
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function reader(world: World): Person {
  const answer = world.login({ login: 'ada', password: 'correct horse battery' });
  if (!record(answer.body) || typeof answer.body.access_token !== 'string') throw new Error('The synthetic browser reader must sign in');
  return world.browserPerson(`Bearer ${answer.body.access_token}`);
}
function canonical(world: World, person: Person): Record<string, unknown> {
  const reply = world.resources.handle(person, 'POST', '/resources/library', new URLSearchParams(), { targets: [target] });
  if (reply === null || !Array.isArray(reply.body) || !record(reply.body[0]) || !record(reply.body[0].facts)) throw new Error('The existing Library producer must return its target facts');
  return reply.body[0].facts;
}
function withoutClock(value: unknown) {
  if (!record(value)) return value;
  const { as_of: _instant, ...rest } = value;
  return rest;
}
async function opening(world: World, person: Person): Promise<Record<string, unknown>> {
  const reply = await world.requestFeeds.read(person, `/environments/${id}/sync`);
  if (!record(reply.body)) throw new Error('The current Environment opening must be an object');
  return reply.body;
}

it('includes exactly the canonical authorized facts in the HTTP opening without inventing recorded totals', async () => {
  const world = new World(), person = reader(world), expected = canonical(world, person);
  const reply = await opening(world, person);
  expect(withoutClock(reply.library)).toEqual(withoutClock(expected));
  expect(record(reply.library) && reply.library.dependents_complete).toBe(false);
  const uses = record(reply.library) ? reply.library.uses : undefined;
  expect(record(uses) && uses.all_time).toBeNull();
  expect(record(uses) && uses.coverage).toEqual({ state: 'unrecorded' });
  expect(reply.jobs).toEqual([]);
});

it('uses the same native principal and withdraws Library facts while the existing file door stays readable', async () => {
  const world = new World(), person = reader(world), expected = canonical(world, person);
  let deliver: (value: Record<string, unknown>) => void = () => undefined;
  const first = new Promise<Record<string, unknown>>((resolve) => { deliver = resolve; });
  const rows: Record<string, unknown>[] = [];
  const subscriber: Subscriber = { send(value) { if (record(value) && value.op === 'sync' && record(value.data)) { rows.push(value.data); deliver(value.data); } }, close: vi.fn() };
  expect(world.requestFeeds.frame(subscriber, person, { op: 'sub', id: 7, feed: `env:${id}` })).toBe(true);
  expect(withoutClock((await first).library)).toEqual(withoutClock(expected));
  world.places.denied.set(person.me.username, new Set([`environment.view@${id}`]));
  rows.length = 0; await world.requestFeeds.changed();
  expect(rows.length).toBeGreaterThan(0);
  expect(rows.every((row) => row.library === null)).toBe(true);
  const current = await opening(world, person);
  expect(current.library).toBeNull();
  const environment = record(current.environment) ? current.environment : undefined;
  const access = record(environment?.access) ? environment.access : undefined;
  expect(access?.permissions).not.toContain('environment.view');
  expect(access?.permissions).toContain('files.view');
  world.requestFeeds.gone(subscriber);
});

it('keeps the canonical Environment View denial and the opening null in agreement', async () => {
  const world = new World(), person = reader(world);
  world.places.denied.set(person.me.username, new Set([`environment.view@${id}`]));
  let refusal: unknown;
  try { canonical(world, person); } catch (error) { refusal = error; }
  expect(refusal).toBeInstanceOf(Refusal);
  if (refusal instanceof Refusal) expect(refusal.answer.status).toBe(404);
  expect((await opening(world, person)).library).toBeNull();
  expect(world.resources.handle(person, 'POST', '/resources/library', new URLSearchParams(), { targets: [{ object: { kind: 'provider', id: RUNS_ON.teamAnthropic }, version_id: null }] })?.status).toBe(200);
});
