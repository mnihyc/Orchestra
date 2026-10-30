// @vitest-environment node
/** Group selectors, directory membership and usage caps share the same authorized fake groups. */
import { expect, it } from 'vitest';
import type { Group, UsageCap } from '@/generated/api-types';
import { ACME } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

it('lists the same native groups in the aggregate reader and directory', async () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const path = `/orgs/${ACME.org}/groups`;
  const direct = world.access.handle(person, 'GET', path, new URLSearchParams(), {})!;
  expect((await world.requestFeeds.read(person, path)).body).toEqual(direct.body);
});

it('does not reveal an org group selector to somebody outside its page', async () => {
  const world = new World();
  const outsider = world.people.get('zoe')!;
  await expect(world.requestFeeds.read(outsider, `/orgs/${ACME.org}/groups`)).rejects.toThrow(Refusal);
});

it('uses a group from the directory for a cap, with its current membership', () => {
  const world = new World();
  const person = world.people.get('ada')!;
  const groups = world.access.handle(person, 'GET', `/orgs/${ACME.org}/groups`, new URLSearchParams(), {})!.body as Group[];
  const backend = groups.find((group) => group.name === 'Backend team')!;
  const vera = world.people.get('vera')!;
  world.access.handle(person, 'PUT', `/groups/${backend.id}/holders`, new URLSearchParams(), { kind: 'user', id: vera.me.id });
  const reply = world.usage.handle(person, 'POST', '/usage/caps', new URLSearchParams(), {
    node_id: ACME.web, meter: 'credits', split: 'each', subject: { kind: 'group', id: backend.id }, period: 'week', amount: '100',
  });
  const cap = reply?.body as UsageCap;
  expect(cap.subject).toMatchObject({ kind: 'group', group: { id: backend.id, name: backend.name, member_count: 4 } });
});
