// @vitest-environment node
/** One world's real session suppliers: no route fulfillment or successful-response injection. */
import { expect, it } from 'vitest';
import type { AuthResult, SessionInfo } from '../../src/generated/api-types.ts';
import { bootstrap } from '../doubles/bootstrap.ts';
import { ACME } from '../doubles/fixtures.ts';
import { Refusal, World } from '../doubles/world.ts';

function login(world: World, username = 'ada', password = 'correct horse battery') {
  const answer = world.login({ login: username, password });
  expect(answer.status).toBe(200);
  const auth = answer.body as AuthResult;
  const cookie = answer.cookies?.['__Host-orch_session'];
  if (typeof cookie !== 'string') throw new Error('The actual login supplier returned no session');
  return { bearer: `Bearer ${auth.access_token}`, cookie, auth };
}

function refused(work: () => unknown, status: number, code: string) {
  try { work(); throw new Error('The request unexpectedly succeeded'); }
  catch (error) {
    if (!(error instanceof Refusal)) throw error;
    expect(error.answer.status).toBe(status);
    expect(error.answer.body).toMatchObject({ code, status });
  }
}

it('keeps two browser switches independent through refresh and logout while identity and work stay unchanged', async () => {
  const world = new World(), a = login(world), b = login(world);
  const original = structuredClone(world.people.get('ada')!.me);
  const work = structuredClone(world.threads.snapshot(ACME.mainThread));
  expect(world.adminMode(a.bearer)).toEqual({ as_admin: false, available: true });
  expect(world.setAdminMode(a.bearer, { as_admin: true }).body).toEqual({ as_admin: true, available: true });
  expect(world.adminMode(b.bearer)).toEqual({ as_admin: false, available: true });
  expect((await bootstrap(world, world.caller(a.bearer), a.bearer)).admin_mode).toEqual(world.adminMode(a.bearer));
  expect((await bootstrap(world, world.caller(b.bearer), b.bearer)).admin_mode).toEqual(world.adminMode(b.bearer));
  world.tokens.get(a.auth.access_token)!.endsAt = 0;
  refused(() => world.adminMode(a.bearer), 401, 'auth.invalid_token');
  const refreshed = world.refresh({ '__Host-orch_session': a.cookie }, '1').body as AuthResult;
  expect(world.adminMode(`Bearer ${refreshed.access_token}`)).toEqual({ as_admin: true, available: true });
  expect(world.logout({ '__Host-orch_session': b.cookie }, '1').status).toBe(204);
  refused(() => world.adminMode(b.bearer), 401, 'auth.invalid_token');
  expect(world.adminMode(`Bearer ${refreshed.access_token}`)).toEqual({ as_admin: true, available: true });
  expect(world.people.get('ada')!.me).toEqual(original);
  expect(world.threads.snapshot(ACME.mainThread)).toEqual(work);
  expect(login(world).auth.user).toEqual(original);
});

it('refuses non-admin, malformed and non-browser enablement without borrowing a different session', () => {
  const world = new World(), admin = login(world), ordinary = login(world, 'dora', 'compilers and goto');
  expect(world.adminMode(ordinary.bearer)).toEqual({ as_admin: false, available: false });
  refused(() => world.setAdminMode(ordinary.bearer, { as_admin: true }), 403, 'authz.denied');
  for (const body of [{}, { as_admin: 'true' }, { as_admin: true, session_id: 'another session' }]) {
    refused(() => world.setAdminMode(admin.bearer, body), 400, 'request.invalid');
  }
  world.tokens.set('synthetic-non-browser', { username: 'ada', endsAt: Date.now() + 60_000 });
  refused(() => world.adminMode('Bearer synthetic-non-browser'), 403, 'auth.session_required');
  refused(() => world.setAdminMode('Bearer synthetic-non-browser', { as_admin: true }), 403, 'auth.session_required');
  expect(world.browserPerson('Bearer synthetic-non-browser').me.is_system_admin).toBe(false);
  expect(world.adminMode(admin.bearer)).toEqual({ as_admin: false, available: true });
  expect(world.setAdminMode(ordinary.bearer, { as_admin: false }).body).toEqual({ as_admin: false, available: false });
  const roles = world.places.roles.get(ACME.org)!;
  roles.set('dora', { ...roles.get('dora')!, role: 'admin' });
  expect(world.setAdminMode(ordinary.bearer, { as_admin: true }).body).toEqual({ as_admin: true, available: true });
  expect(world.browserPerson(ordinary.bearer).me.is_system_admin).toBe(false);
});

it('uses current session reach for a private canary, keeps the raw identity and gives a peer no borrowed read', () => {
  const world = new World(), a = login(world), b = login(world);
  const made = world.documents.control('/make', { user: 'dora', node: ACME.fixLogin, title: 'Session canary', private: true, count: 1 })?.body as { id: string };
  const actor = world.browserPerson(a.bearer), peer = world.browserPerson(b.bearer);
  const read = (reader: typeof actor) => world.documents.handle(reader, 'GET', `/documents/${made.id}`, new URLSearchParams(), {});
  refused(() => read(actor), 404, 'not_found');
  world.updateMe(actor, { theme: 'dark' });
  expect(world.people.get('ada')!.me.theme).toBe('dark');
  expect(world.caller(a.bearer).me.is_system_admin).toBe(true);
  world.setAdminMode(a.bearer, { as_admin: true });
  expect(read(actor)?.status).toBe(200);
  refused(() => read(peer), 404, 'not_found');
  world.setAdminMode(a.bearer, { as_admin: false });
  refused(() => read(actor), 404, 'not_found');
  expect(world.people.get('ada')!.me.is_system_admin).toBe(true);
});

it('rechecks revoked roles and suspension without widening ordinary roles or transferring a stored switch', () => {
  const world = new World(), signed = login(world);
  const actor = world.browserPerson(signed.bearer);
  world.setAdminMode(signed.bearer, { as_admin: true });
  const ada = world.people.get('ada')!;
  ada.me = { ...ada.me, is_system_admin: false };
  for (const roles of world.places.roles.values()) roles.delete('ada');
  expect(world.adminMode(signed.bearer)).toEqual({ as_admin: true, available: false });
  expect(actor.me.is_system_admin).toBe(false);
  refused(() => world.setAdminMode(signed.bearer, { as_admin: true }), 403, 'authz.denied');
  expect(world.setAdminMode(signed.bearer, { as_admin: false }).body).toEqual({ as_admin: false, available: false });
  ada.suspended = true;
  refused(() => world.adminMode(signed.bearer), 401, 'auth.suspended');
  expect(world.tokenUser(signed.auth.access_token)).toBeNull();
});

it('announces the actual source session ID and revokes only the chosen peer through the existing session route', () => {
  const world = new World(), a = login(world), b = login(world);
  const actor = world.browserPerson(a.bearer), peer = world.browserPerson(b.bearer);
  const events: unknown[] = [];
  const inboxSeq = world.notifications.seq('ada');
  const subscriber = { send: (frame: unknown) => { events.push(frame); }, close: () => undefined };
  world.users.frame(subscriber, actor, { op: 'sub', id: 7, feed: 'user' });
  world.setAdminMode(a.bearer, { as_admin: true });
  world.setAdminMode(a.bearer, { as_admin: true });
  const rows = world.settings.handle(actor, 'GET', '/me/sessions', new URLSearchParams(), {})?.body as SessionInfo[];
  const current = rows.find((entry) => entry.current)!;
  const other = world.browserSessionRefs(peer)!.find((entry) => entry.current)!;
  expect(current.id).not.toBe(other.id);
  expect(events).toEqual([
    { op: 'subscribed', id: 7, feed: 'user', seq: inboxSeq, mode: 'delta' },
    { op: 'event', id: 7, feed: 'user', data: { what: 'admin_mode', session_id: current.id, as_admin: true } },
  ]);
  expect(world.settings.handle(actor, 'DELETE', `/me/sessions/${other.id}`, new URLSearchParams(), {})?.status).toBe(204);
  refused(() => world.adminMode(b.bearer), 401, 'auth.invalid_token');
  expect(world.adminMode(a.bearer)).toEqual({ as_admin: true, available: true });
  world.users.gone(subscriber);
});

it('resets a fresh login to Off and ends all retained credentials when the world is reset', () => {
  const world = new World(), a = login(world);
  world.setAdminMode(a.bearer, { as_admin: true });
  const b = login(world);
  expect(world.adminMode(b.bearer)).toEqual({ as_admin: false, available: true });
  world.reset();
  refused(() => world.adminMode(a.bearer), 401, 'auth.invalid_token');
  expect(world.adminMode(login(world).bearer)).toEqual({ as_admin: false, available: true });
});
