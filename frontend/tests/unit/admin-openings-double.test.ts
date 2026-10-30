// @vitest-environment node
/** Real shared document/session/trace suppliers; no audit row is injected. */
import { randomUUID } from 'node:crypto';
import { expect, it } from 'vitest';
import type { AuthResult, AuditEntryPage, Document } from '../../src/generated/api-types.ts';
import { ACME } from '../doubles/fixtures.ts';
import { Refusal, World } from '../doubles/world.ts';

function login(world: World) {
  const reply = world.login({ login: 'ada', password: 'correct horse battery' });
  expect(reply.status).toBe(200);
  const auth = reply.body as AuthResult;
  return { bearer: `Bearer ${auth.access_token}`, cookie: reply.cookies!.orch_session! };
}
function document(world: World, user = 'dora') {
  const reply = world.documents.control('/make', { user, node: ACME.fixLogin, title: `Private opening ${randomUUID()}`, private: true, count: 1 });
  expect(reply?.status).toBe(200);
  return (reply!.body as { id: string }).id;
}
function scene() {
  const world = new World(), signed = login(world), id = document(world);
  world.setAdminMode(signed.bearer, { as_admin: true });
  const actor = world.browserPerson(signed.bearer);
  const rows = () => world.trace.records.filter(({ entry }) => entry.action === 'admin.opened');
  const read = (key = id) => world.documents.handle(actor, 'GET', `/documents/${key}`, new URLSearchParams(), {})!.body as Document;
  return { world, signed, id, actor, rows, read };
}
function refused(work: () => unknown, status: number, code: string) {
  try { work(); throw new Error('The request unexpectedly succeeded'); }
  catch (error) {
    if (!(error instanceof Refusal)) throw error;
    expect(error.answer.status).toBe(status);
    expect(error.answer.body).toMatchObject({ status, code });
  }
}

it('writes the exact native private-opening action and details through the actual trace reader', async () => {
  const s = scene(), nonce = randomUUID();
  const value = s.read();
  expect(value.access.admin_access).toEqual({ place: 'system', private: true });
  s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${s.id}`, new URLSearchParams(), value, nonce, 'real-reader-request');
  const session = s.world.browserSessionRefs(s.actor)!.find((row) => row.current)!.id;
  const { entry, ancestry } = s.rows()[0]!;
  expect(ancestry).toEqual([]);
  expect(entry).toMatchObject({ action: 'admin.opened', kind: 'access', as_admin: true, history: false, override: false,
    actor: { kind: 'user', user: { id: s.actor.me.id } }, node: null,
    object: { kind: 'document', id: s.id, name: value.title, can_open: true },
    summary: 'opened a thing through administrative access', request_id: 'real-reader-request', ip: null });
  expect(entry.details).toEqual({ place: 'system', session_id: session, opening_id: nonce });
  const page = (await s.world.trace.handle(s.actor, 'GET', '/admin/audit', new URLSearchParams({ action: 'admin.opened' }), {}))!.body as AuditEntryPage;
  expect(page.items).toEqual([entry]);
  const scoped = (await s.world.trace.handle(s.actor, 'GET', `/nodes/${ACME.fixLogin}/audit`, new URLSearchParams({ action: 'admin.opened' }), {}))!.body as AuditEntryPage;
  expect(scoped.items).toEqual([]);
});

it('joins actual HTTP and cached reads by the full key without replacing the canonical row', () => {
  const s = scene(), nonce = randomUUID();
  s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${s.id}`, new URLSearchParams(), s.read(), nonce, 'first-request');
  const original = structuredClone(s.rows()[0]);
  expect(s.world.openings.cached(s.signed.bearer, { kind: 'document', id: s.id, opening_id: nonce }, 'retry-request')).toEqual({ status: 204 });
  s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${s.id}`, new URLSearchParams(), s.read(), nonce.toUpperCase(), 'another-http-request');
  expect(s.rows()).toEqual([original]);
  const peer = login(s.world);
  s.world.setAdminMode(peer.bearer, { as_admin: true });
  s.world.openings.cached(peer.bearer, { kind: 'document', id: s.id, opening_id: nonce });
  s.world.openings.cached(s.signed.bearer, { kind: 'document', id: s.id, opening_id: randomUUID() });
  const other = document(s.world);
  s.world.openings.cached(s.signed.bearer, { kind: 'document', id: other, opening_id: nonce });
  expect(s.rows()).toHaveLength(4);
  expect(s.rows().find(({ entry }) => entry.id === original!.entry.id)).toEqual(original);
});

it('does not turn an ordinary or failed read, a list, or deleted inspection into a private opening', () => {
  const s = scene(), nonce = randomUUID(), own = document(s.world, 'ada');
  const value = s.read(own);
  expect(value.access.admin_access).toBeNull();
  s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${own}`, new URLSearchParams(), value, nonce, null);
  s.world.openings.cached(s.signed.bearer, { kind: 'document', id: own, opening_id: nonce });
  s.world.openings.finishJson(s.signed.bearer, 'GET', `/nodes/${ACME.fixLogin}/documents`, new URLSearchParams(), { items: [s.read()] }, nonce, null);
  s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${s.id}`, new URLSearchParams({ inspect_deleted: 'true' }), s.read(), nonce, null);
  refused(() => s.read(randomUUID()), 404, 'not_found');
  expect(s.rows()).toEqual([]);
});

it('validates identities and explicitly refuses unsupported authorization instead of acknowledging it', () => {
  const s = scene(), body = { kind: 'document', id: s.id, opening_id: randomUUID() };
  for (const changed of [{ ...body, id: 'not-an-id' }, { ...body, opening_id: 'a display name' }, { ...body, kind: 'doc' }, { ...body, session_id: randomUUID() }]) {
    refused(() => s.world.openings.cached(s.signed.bearer, changed), 400, 'request.invalid');
  }
  refused(() => s.world.openings.cached(s.signed.bearer, { ...body, kind: 'environment' }), 400, 'request.value_refused');
  refused(() => s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${s.id}`, new URLSearchParams(), s.read(), 'bad nonce', null), 400, 'request.invalid');
  const roles = s.world.places.roles.get(ACME.org)!;
  roles.set('dora', { ...roles.get('dora')!, role: 'admin' });
  const signed = s.world.login({ login: 'dora', password: 'compilers and goto' }).body as AuthResult;
  const scoped = `Bearer ${signed.access_token}`, other = document(s.world, 'grace');
  s.world.setAdminMode(scoped, { as_admin: true });
  refused(() => s.world.openings.cached(scoped, { ...body, id: other }), 400, 'request.value_refused');
  expect(s.rows()).toEqual([]);
});

it('rechecks mode, live credentials and current document authority before considering a retained nonce', () => {
  const s = scene(), body = { kind: 'document', id: s.id, opening_id: randomUUID() };
  s.world.setAdminMode(s.signed.bearer, { as_admin: false });
  expect(s.world.openings.cached(s.signed.bearer, body)).toEqual({ status: 204 });
  refused(() => s.read(), 404, 'not_found');
  expect(s.rows()).toEqual([]);
  s.world.setAdminMode(s.signed.bearer, { as_admin: true });
  s.world.openings.cached(s.signed.bearer, body);
  const original = structuredClone(s.rows());
  const raw = s.world.people.get('ada')!;
  raw.me = { ...raw.me, is_system_admin: false };
  for (const roles of s.world.places.roles.values()) roles.delete('ada');
  expect(s.world.openings.cached(s.signed.bearer, body)).toEqual({ status: 204 });
  refused(() => s.read(), 404, 'not_found');
  expect(s.rows()).toEqual(original);
  s.world.logout({ orch_session: s.signed.cookie }, '1');
  refused(() => s.world.openings.cached(s.signed.bearer, body), 401, 'auth.invalid_token');
  expect(s.rows()).toEqual(original);
});

it('keeps browser provenance required and gives nonce-less successful private reads independent openings', () => {
  const s = scene();
  s.world.tokens.set('synthetic-non-browser', { username: 'ada', endsAt: Date.now() + 60_000 });
  refused(() => s.world.openings.cached('Bearer synthetic-non-browser', { kind: 'document', id: s.id, opening_id: randomUUID() }), 403, 'auth.session_required');
  for (let read = 0; read < 2; read += 1) s.world.openings.finishJson(s.signed.bearer, 'GET', `/documents/${s.id}`, new URLSearchParams(), s.read(), undefined, null);
  expect(s.rows()).toHaveLength(2);
  const details = s.rows().map(({ entry }) => entry.details as { opening_id: string });
  expect(details[0]!.opening_id).not.toBe(details[1]!.opening_id);
});
