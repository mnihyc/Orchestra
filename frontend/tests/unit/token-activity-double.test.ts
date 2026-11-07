// @vitest-environment node
/** Source fixtures prove projection and cursor parity, not native key execution. */
import { expect, it } from 'vitest';
import type { ApiTokenActivity } from '../../src/generated/api-types';
import { ACME, id } from '../doubles/fixtures';
import { World } from '../doubles/world';

function call(number: number): ApiTokenActivity {
  return { id: id(0x6f, number), at: new Date(Date.now() - number * 1000).toISOString(), operation: 'NodeGet', selection: 'NodeGet.read', method: 'GET', route: '/nodes/{id}', status: 200, error_code: null, replayed: false, duration_ms: 2, request_id: id(0x70, number), ip: '203.0.113.12',
    facets: [{ operation: 'NodeGet.read', action: 'read', resource_kind: 'key', target: { kind: 'key', id: id(0x71, number) }, target_name: 'untrusted historical name' }] };
}

it('keeps personal ownership and current-name privacy without trusting historical labels', () => {
  const world = new World(), person = world.people.get('ada')!, other = world.people.get('dora')!, key = id(0x72, 1);
  world.settings.tokenActivity.remember(key, person.me.id);
  world.settings.tokenActivity.seed(key, [{ row: call(1), organizations: [[ACME.org]] }]);
  const page = world.settings.tokenActivity.page(person, key, undefined, new URLSearchParams());
  expect(page.items[0]?.facets[0]?.target_name).toBeNull();
  expect(page.items[0]?.ip).toBe('203.0.113.12');
  expect(() => world.settings.tokenActivity.page(other, key, undefined, new URLSearchParams())).toThrow();
});

it('forwards a key/org-bound snapshot and filters each facet before returning an org slice', () => {
  const world = new World(), person = world.people.get('ada')!, key = id(0x72, 2);
  const personal = person.me.personal_org_id;
  if (personal === null) throw new Error('This human fixture has a personal org');
  world.settings.tokenActivity.remember(key, person.me.id);
  const one = call(1); one.facets.push({ ...one.facets[0]!, target: { kind: 'key', id: id(0x71, 5) } });
  world.settings.tokenActivity.seed(key, [{ row: one, organizations: [[ACME.org], [personal]] }, { row: call(2), organizations: [[ACME.org]] }]);
  const first = world.settings.tokenActivity.page(person, key, ACME.org, new URLSearchParams({ limit: '1' }));
  expect(first.items[0]?.facets).toHaveLength(1);
  if (first.next_cursor === null) throw new Error('The second page must have a native-shaped cursor');
  world.settings.tokenActivity.record(key, { row: call(0), organizations: [[ACME.org]] });
  expect(world.settings.tokenActivity.page(person, key, ACME.org, new URLSearchParams({ limit: '1', cursor: first.next_cursor })).items[0]?.id).toBe(id(0x6f, 2));
  expect(() => world.settings.tokenActivity.page(person, key, undefined, new URLSearchParams({ cursor: first.next_cursor ?? '' }))).toThrow();
});

it('keeps the server seven-day/hundred-call window without fabricating activity for a new key', () => {
  const world = new World(), person = world.people.get('ada')!, key = id(0x72, 3);
  world.settings.tokenActivity.remember(key, person.me.id);
  expect(world.settings.tokenActivity.page(person, key, undefined, new URLSearchParams()).items).toEqual([]);
  const old = call(1); old.at = new Date(Date.now() - 8 * 86_400_000).toISOString();
  world.settings.tokenActivity.seed(key, [{ row: old, organizations: [[ACME.org]] }]);
  expect(world.settings.tokenActivity.page(person, key, undefined, new URLSearchParams()).items).toEqual([]);
  world.settings.tokenActivity.seed(key, Array.from({ length: 101 }, (_, index) => ({ row: call(index + 1), organizations: [[ACME.org]] })));
  expect(world.settings.tokenActivity.page(person, key, undefined, new URLSearchParams({ limit: '100' })).items).toHaveLength(100);
});
