/** Native UserGet projects one public identity by exact ID after the server's existing authentication gate. */
import { expect, it } from 'vitest';
import { id, userRef } from '../doubles/fixtures';
import { Refusal, World } from '../doubles/world';

it('resolves a committed identity for an ordinary reader without admin or org search authority', () => {
  const world = new World(), viewer = world.people.get('vera'), selected = world.people.get('dora');
  if (viewer === undefined || selected === undefined) throw new Error('The existing fixture people are required.');
  expect(viewer.me.is_system_admin).toBe(false);
  const answer = world.places.handle(viewer, 'GET', `/users/${selected.me.id}`, new URLSearchParams(), {});
  expect(answer).toEqual({ status: 200, body: { ...userRef(selected.me), address: '@dora' } });
  expect(Object.keys(answer?.body as object).sort()).toEqual(['address', 'avatar', 'display_name', 'id', 'kind', 'username']);
});

it('retains exact identity lookup for suspended recorded actors without exposing account or credential fields', () => {
  const world = new World(), viewer = world.people.get('vera'), selected = world.people.get('dora');
  if (viewer === undefined || selected === undefined) throw new Error('The existing fixture people are required.');
  selected.suspended = true;
  const answer = world.places.handle(viewer, 'GET', `/users/${selected.me.id}`, new URLSearchParams(), {});
  expect(answer?.body).toEqual({ ...userRef(selected.me), address: '@dora' });
  expect(answer?.body).not.toHaveProperty('email'); expect(answer?.body).not.toHaveProperty('password');
  expect(answer?.body).not.toHaveProperty('is_system_admin'); expect(answer?.body).not.toHaveProperty('suspended');
});

it('returns not_found for an absent ID and keeps name search, methods and paths separate', () => {
  const world = new World(), viewer = world.people.get('vera');
  if (viewer === undefined) throw new Error('The existing viewer is required.');
  let missing: unknown;
  try { world.places.handle(viewer, 'GET', `/users/${id(1, 999)}`, new URLSearchParams(), {}); } catch (error) { missing = error; }
  expect(missing).toBeInstanceOf(Refusal); expect(missing).toMatchObject({ answer: { status: 404, body: { code: 'not_found' } } });
  expect(world.places.handle(viewer, 'GET', '/users/search', new URLSearchParams({ q: 'dora' }), {})?.body)
    .toEqual([userRef(world.people.get('dora')!.me)]);
  expect(world.places.handle(viewer, 'GET', '/users', new URLSearchParams(), {})).toBeNull();
  expect(world.places.handle(viewer, 'GET', `/users/${viewer.me.id}/other`, new URLSearchParams(), {})).toBeNull();
  expect(world.places.handle(viewer, 'POST', `/users/${viewer.me.id}`, new URLSearchParams(), {})).toBeNull();
  expect(() => world.browserPerson(undefined)).toThrow(Refusal);
});
