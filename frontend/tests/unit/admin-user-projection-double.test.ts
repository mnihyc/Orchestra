// @vitest-environment node
/** The existing native endpoint has one authoritative double projection, filtering before username cursors. */
import { expect, it, vi } from 'vitest';
import type { AdminUserPage, AuthResult } from '../../src/generated/api-types.ts';
import { Refusal, World } from '../doubles/world.ts';

function setup() {
  const world = new World(), login = world.login({ login: 'ada', password: 'correct horse battery' });
  const auth = login.body as AuthResult, bearer = `Bearer ${auth.access_token}`;
  world.setAdminMode(bearer, { as_admin: true });
  const actor = world.browserPerson(bearer);
  const list = (query = new URLSearchParams()) => world.admin.handle(actor, 'GET', '/admin/users', query, {})!.body as AdminUserPage;
  return { world, actor, list, bearer };
}

it('leaves GET /admin/users to the actual admin handler and retains locks, suspension and invitation facts', async () => {
  const { world, actor, list } = setup();
  expect(await world.trace.handle(actor, 'GET', '/admin/users', new URLSearchParams(), {})).toBeNull();
  const locked = list(new URLSearchParams({ q: 'bolted' })).items[0];
  expect(locked?.locked).toEqual([expect.objectContaining({ way: 'password' })]);
  const target = world.people.get('linus'); if (target === undefined) throw new Error('The standing fixture is missing');
  world.admin.handle(actor, 'PATCH', `/admin/users/${target.me.id}`, new URLSearchParams(), { status: 'suspended' });
  expect(list(new URLSearchParams({ q: 'linus', status: 'suspended' })).items).toEqual([expect.objectContaining({ id: target.me.id, status: 'suspended' })]);
  expect(list(new URLSearchParams({ q: 'linus', status: 'active' })).items).toEqual([]);
  const tell = vi.spyOn(world.users, 'tell');
  expect(world.admin.handle(actor, 'POST', `/admin/users/${target.me.id}/org-invitation`, new URLSearchParams(), {})?.status).toBe(204);
  expect(list(new URLSearchParams({ q: 'linus' })).items[0]?.org_invited_at).not.toBeNull();
  expect(tell).toHaveBeenCalledExactlyOnceWith('linus', { what: 'org_creation' });
  world.admin.handle(actor, 'POST', `/admin/users/${target.me.id}/org-invitation`, new URLSearchParams(), {}); expect(tell).toHaveBeenCalledOnce();
  world.admin.handle(actor, 'DELETE', `/admin/users/${target.me.id}/org-invitation`, new URLSearchParams(), {});
  expect(list(new URLSearchParams({ q: 'linus' })).items[0]?.org_invited_at).toBeNull(); expect(tell).toHaveBeenCalledTimes(2);
});

it('uses username ordering/cursors and native q, status and system_admin predicates before the limit', () => {
  const { world, list } = setup();
  const usernames: string[] = []; let cursor: string | null = null;
  do {
    const result = list(new URLSearchParams({ limit: '2', system_admin: 'false', ...(cursor === null ? {} : { cursor }) }));
    expect(result.items.every((row) => !row.is_system_admin)).toBe(true); usernames.push(...result.items.map((row) => row.username));
    if (result.next_cursor !== null) expect(result.next_cursor).toBe(result.items.at(-1)?.username);
    cursor = result.next_cursor;
  } while (cursor !== null);
  const expected = [...world.people.values()].filter((row) => row.me.kind === 'human' && !row.me.is_system_admin).map((row) => row.me.username).sort();
  expect(usernames).toEqual(expected); expect(new Set(usernames).size).toBe(usernames.length);
  const person = world.people.get('vera'), email = person?.me.email;
  if (person === undefined || typeof email !== 'string') throw new Error('The email fixture is missing');
  expect(list(new URLSearchParams({ q: `  ${email.toUpperCase()}  `, limit: '1' })).items[0]?.id).toBe(person.me.id);
});

it('keeps the native browser administrative-mode gate after removing the shadow handler', () => {
  const { world, bearer } = setup(); world.setAdminMode(bearer, { as_admin: false });
  const actor = world.browserPerson(bearer);
  expect(() => world.admin.handle(actor, 'GET', '/admin/users', new URLSearchParams(), {})).toThrow(Refusal);
});

it('sends only a bare metadata hint for a changed contributing system setting', () => {
  const { world, actor } = setup(), tell = vi.spyOn(world.users, 'tellAll');
  world.admin.handle(actor, 'PUT', '/admin/settings/system.banner', new URLSearchParams(), { value: { text: 'A notice', level: 'info', until: null } });
  expect(tell).toHaveBeenCalledExactlyOnceWith({ what: 'meta_changed' });
  world.admin.handle(actor, 'PUT', '/admin/settings/hosts.full_percent', new URLSearchParams(), { value: 80 }); expect(tell).toHaveBeenCalledOnce();
  world.admin.handle(actor, 'DELETE', '/admin/settings/system.banner', new URLSearchParams(), {}); expect(tell).toHaveBeenCalledTimes(2);
});

it('keeps the native HTTP request cause on metadata hints without reusing the idempotency key', () => {
  const { world, actor } = setup(), tell = vi.spyOn(world.users, 'tellAll');
  world.admin.handle(actor, 'PUT', '/admin/settings/system.banner', new URLSearchParams(),
    { value: { text: 'Causal notice', level: 'info', until: null } }, 'idempotency-only', 'native-response-id');
  expect(tell).toHaveBeenCalledExactlyOnceWith({ what: 'meta_changed', request_id: 'native-response-id' });
});
