/** Browser double contract: the existing authorized node-history endpoint filters actors before paging. */
import { expect, it } from 'vitest';
import type { HistoryLinePage } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { Refusal, World, type Person } from '../doubles/world';

function scene() {
  const world = new World(), ada = world.people.get('ada'), grace = world.people.get('grace');
  if (ada === undefined || grace === undefined) throw new Error('Both existing people are required.');
  // Existing fixture control records real access-history rows without exposing or mutating its private row map.
  for (const as of ['grace', 'ada', 'grace', 'grace', 'ada', 'ada']) {
    expect(world.access.control('/touch', { as, id: ACME.mainThread, change: { rows: [{ permission: 'thread.view', inherit: true }] } })?.status).toBe(200);
  }
  const read = (query: Record<string, string> = {}, person: Person = ada): HistoryLinePage => {
    const answer = world.access.handle(person, 'GET', `/nodes/${ACME.mainThread}/history`, new URLSearchParams(query), {});
    if (answer?.status !== 200 || answer.body === undefined) throw new Error('The existing node history endpoint must return its ordinary page.');
    return answer.body as HistoryLinePage;
  };
  return { world, ada, grace, read };
}

it('finds an actor absent from the unfiltered first page and continues within that filtered sequence', () => {
  const { grace, read } = scene();
  const all = read(), initial = read({ limit: '2' });
  expect(initial.items).toHaveLength(2);
  expect(initial.items.every((line) => line.actor !== null && !('agent' in line.actor) && line.actor.id !== grace.me.id)).toBe(true);
  const expected = all.items.filter((line) => line.actor !== null && !('agent' in line.actor) && line.actor.id === grace.me.id);
  expect(expected).toHaveLength(3);
  const first = read({ actor: grace.me.id, limit: '2' });
  expect(first.items).toEqual(expected.slice(0, 2)); expect(first.next_cursor).toBe('2');
  const second = read({ actor: grace.me.id, limit: '2', cursor: first.next_cursor! });
  expect(second.items).toEqual(expected.slice(2)); expect(second.next_cursor).toBeNull();
  expect([...first.items, ...second.items]).toEqual(expected);
  expect(read()).toEqual(all);
});

it('preserves no-actor ordering/projection and returns no cursor for an unmatched actor', () => {
  const { ada, grace, read } = scene();
  const all = read();
  expect(all.items.map((line) => line.actor !== null && !('agent' in line.actor) ? line.actor.id : null))
    .toEqual([ada.me.id, ada.me.id, grace.me.id, grace.me.id, ada.me.id, grace.me.id, ada.me.id]);
  expect(all.items.every((line) => line.about === null)).toBe(true); expect(all.items.at(-1)?.action).toBe('node.created');
  const first = read({ limit: '2' }), next = read({ limit: '2', cursor: first.next_cursor! });
  expect([...first.items, ...next.items]).toEqual(all.items.slice(0, 4));
  expect(read({ actor: '00000000-0000-0000-0000-000000000000', limit: '2' })).toEqual({ items: [], next_cursor: null });
});

it('checks the same target authority before an actor filter and does not broaden supported fixture scopes', () => {
  const { world, ada, grace, read } = scene();
  world.places.denied.set(grace.me.username, new Set([`thread.view@${ACME.mainThread}`]));
  const queries: Record<string, string>[] = [{}, { actor: ada.me.id }, { actor: grace.me.id }];
  for (const query of queries) {
    let caught: unknown;
    try { read(query, grace); } catch (error) { caught = error; }
    expect(caught).toBeInstanceOf(Refusal); expect(caught).toMatchObject({ answer: { status: 403, body: { code: 'authz.denied' } } });
  }
  expect(world.access.handle(ada, 'GET', `/documents/${ACME.mainThread}/history`, new URLSearchParams({ actor: ada.me.id }), {})).toBeNull();
  let unsupported: unknown;
  try { world.access.handle(ada, 'GET', `/providers/${ACME.mainThread}/history`, new URLSearchParams({ actor: ada.me.id }), {}); } catch (error) { unsupported = error; }
  expect(unsupported).toBeInstanceOf(Refusal); expect(unsupported).toMatchObject({ answer: { status: 500, body: { code: 'internal' } } });
});
