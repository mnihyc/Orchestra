// @vitest-environment node
/** Participation keeps ordinary ownership and descendant follows distinct from inherited View. */
import { expect, it } from 'vitest';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

it('retains own work and follows below a place, without counting a deleted branch or inherited View', () => {
  const world = new World(), owner = world.people.get('ada'), viewer = world.people.get('vera');
  if (owner === undefined || viewer === undefined) throw new Error('The participation fixture needs both readers');
  world.places.subscriptions.clear(); world.chats.states.clear();
  expect(world.places.participates(owner, ACME.web)).toBe(true);
  expect(world.places.participates(viewer, ACME.web)).toBe(false);
  world.places.subscriptions.set(`${viewer.me.username}:${ACME.mainThread}`, 'all');
  expect(world.places.participates(viewer, ACME.web)).toBe(true);
  world.changeTree(ACME.org, [{ id: ACME.fixLogin, deleted: true }]);
  expect(world.places.participates(viewer, ACME.web)).toBe(false);
  expect(world.places.participates(viewer, 'missing')).toBe(false);
});
