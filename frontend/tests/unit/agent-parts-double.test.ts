// @vitest-environment node
/** Per-area source availability comes from the double's ordinary rows and locks, not its admin view. */
import { expect, it } from 'vitest';

import type { Entry, Holder, ShareParts } from '../../src/generated/api-types';
import type { StoredPage } from '../doubles/access';
import { ACME, id, KIND, userRef } from '../doubles/fixtures';
import { type Person, World } from '../doubles/world';

function entry(person: Person, holder: Holder = { kind: 'user', user: userRef(person.me) }): Entry {
  return {
    id: id(0x6d, 1), holder, options: {}, outside: false, read_first: false,
    expires_at: null, note: '', source: 'page', created_by: userRef(person.me),
    created_at: '2026-10-01T09:00:00.000Z', revision: 1,
  };
}

function clear(page: StoredPage): void {
  for (const row of page.rows.values()) {
    row.inherit = false;
    row.entries = [];
    row.locked = false;
  }
}

function name(page: StoredPage, permission: string, person: Person, profile = 'any'): void {
  const row = page.rows.get(`${permission}|${profile}`);
  if (row === undefined) throw new Error(`The fixture has no ${permission}/${profile} row`);
  row.entries = [entry(person)];
}

function source(world: World, object = ACME.fixLogin, path = 'nodes'): { person: Person; page: StoredPage } {
  const person = world.people.get('ada')!, owner = world.people.get('grace')!;
  person.me = { ...person.me, is_system_admin: false };
  world.places.roles.get(ACME.org)?.delete(person.me.username);
  if (path === 'nodes') world.places.metaOf(object).owner = owner.me.username;
  const page = world.access.pageFor(object, false, path);
  page.owners = [entry(owner)];
  clear(page);
  if (['org', 'project', 'topic'].includes(page.kind)) clear(world.access.pageFor(object, true));
  // Admission is an explicit ordinary View; it supplies no document/chat/thread write rights.
  name(page, page.kind === 'document' ? 'doc.view' : `${page.kind}.view`, person);
  return { person, page };
}

function read(world: World, person: Person, object: string, path = 'nodes', defaults = false) {
  const answer = world.access.handle(person, 'GET', `/${path}/${object}/${defaults ? 'defaults' : 'permissions'}`, new URLSearchParams(), {});
  expect(answer?.status).toBe(200);
  return answer?.body;
}

function gives(body: unknown, parts: ShareParts): void {
  const expectedGives: unknown = expect.objectContaining({ agent_parts: parts });
  expect(body).toEqual(expect.objectContaining({ gives: expectedGives }));
}

it('retains a document-only Read contribution without requiring a whole agent preset', () => {
  const world = new World(), { person, page } = source(world);
  name(page, 'doc.view', person);
  gives(read(world, person, page.id), { documents: 'read' });
});

it('keeps Read when a source lock above prevents giving document Edit', () => {
  const world = new World(), { person, page } = source(world);
  name(page, 'doc.view', person); name(page, 'doc.edit', person);
  const locked = world.access.pageFor(ACME.web, true).rows.get('doc.edit|any')!;
  locked.inherit = false; locked.entries = []; locked.locked = true;
  const body = read(world, person, page.id);
  gives(body, { documents: 'read' });
  const expectedRows: unknown = expect.arrayContaining([expect.objectContaining({ permission: 'doc.edit', can_give: false, locked: { by: { id: ACME.web, kind: 'project', name: 'Web', redacted: false }, here: false } })]);
  expect(body).toMatchObject({ rows: expectedRows });
});

it('keeps human admin page management while refusing an admin-only agent contribution', () => {
  const world = new World(), { person, page } = source(world);
  person.me = { ...person.me, is_system_admin: true };
  const row = page.rows.get('doc.edit|any')!;
  row.entries = [entry(person, { kind: 'role', role: 'admin', at: null, count: 1 })];
  const body = read(world, person, page.id);
  const expectedRows: unknown = expect.arrayContaining([expect.objectContaining({ permission: 'doc.edit', can_give: true })]);
  expect(body).toMatchObject({ owners_writable: true, gives: { roles: ['viewer', 'interactor', 'developer', 'admin'] }, rows: expectedRows });
  gives(body, {});
});

it('uses either ordinary follower profile for a place Timeline instead of live held rows', () => {
  const world = new World(), { person, page } = source(world);
  name(page, 'doc.edit', person);
  gives(read(world, person, page.id), { documents: 'edit' });
  name(world.access.pageFor(page.id, true), 'thread.interact', person, 'connected');
  gives(read(world, person, page.id), { timeline: 'write', documents: 'edit' });
});

it('projects the direct Thread timeline, chat and document levels independently', () => {
  const world = new World(), { person, page } = source(world, ACME.mainThread);
  name(page, 'thread.interact', person); name(page, 'chat.view', person); name(page, 'doc.manage', person);
  gives(read(world, person, page.id), { timeline: 'write', chat: 'read', documents: 'manage' });
});

it('projects only the document area on a document permission page', () => {
  const world = new World(), { person, page } = source(world, id(7, 1), 'documents');
  name(page, 'doc.edit', person);
  gives(read(world, person, page.id, 'documents'), { documents: 'edit' });
});

it('emits no agent parts for defaults or a resource page', () => {
  const world = new World(), person = world.people.get('ada')!;
  gives(read(world, person, ACME.fixLogin, 'nodes', true), {});
  gives(read(world, person, id(KIND.provider, 1), 'providers'), {});
});

it('emits no agent parts when the ordinary holder may not share here', () => {
  const world = new World(), { person, page } = source(world);
  name(page, 'doc.edit', person);
  world.access.control('/holders-share', { place: page.id, allowed: false });
  expect(world.places.metaOf(page.id).owner).toBe('grace');
  expect(world.access.roleOf(person, page.id)).toBeNull();
  expect(world.access.holdingOn(person, page.id)).toMatchObject({ mayShare: false, manages: false });
  const body = read(world, person, page.id);
  expect(body).toMatchObject({ owners_writable: false, gives: { holders_share: false } });
  gives(body, {});
});
