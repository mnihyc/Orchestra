// @vitest-environment node
/** The browser double offers only permitted people and replays a chat grant once. */
import { noticeKind } from '../doubles/notifications.ts';
import { expect, it } from 'vitest';
import type { Chat, UserRef } from '../../src/generated/api-types.ts';
import { ACME, id, me } from '../doubles/fixtures.ts';
import { Refusal, World } from '../doubles/world.ts';

it('puts readers first and excludes an outsider when sharing outside is off', () => {
  const world = new World();
  const ada = world.people.get('ada')!;
  world.access.ownersMayShareOutside = false;
  world.access.noOutsiders.add('ada');
  const query = new URLSearchParams();
  const candidates = world.peopleWork.handle(ada, 'GET', `/documents/${id(7, 3)}/mentions`, query, {})?.body as { items: { user: UserRef; can_view: boolean }[] };
  const firstOutside = candidates.items.findIndex((candidate) => !candidate.can_view);
  if (firstOutside !== -1) expect(candidates.items.slice(firstOutside).every((candidate) => !candidate.can_view)).toBe(true);
  const outsider = world.people.get('zoe')!;
  expect(world.documents.readable(outsider).some((doc) => doc.id === id(7, 3))).toBe(false);
  expect((world.access.handle(ada, 'GET', `/documents/${id(7, 3)}/permissions`, query, {})?.body as { external: boolean }).external).toBe(false);
  const named = world.peopleWork.handle(ada, 'GET', `/documents/${id(7, 3)}/mentions`, new URLSearchParams({ q: outsider.me.username }), {})?.body as { items: { user: UserRef }[] };
  expect(named.items.some((candidate) => candidate.user.id === outsider.me.id)).toBe(false);
});

it('continues a bounded mention page without repeating the first person', () => {
  const world = new World();
  const ada = world.people.get('ada')!, path = `/documents/${id(7, 1)}/mentions`;
  const first = world.peopleWork.handle(ada, 'GET', path, new URLSearchParams({ limit: '1' }), {})?.body as { items: { user: UserRef }[]; next_cursor: string | null };
  expect(first.items).toHaveLength(1);
  expect(first.next_cursor).not.toBeNull();
  const second = world.peopleWork.handle(ada, 'GET', path, new URLSearchParams({ limit: '1', cursor: first.next_cursor ?? '' }), {})?.body as { items: { user: UserRef }[] };
  expect(second.items).toHaveLength(1);
  expect(second.items[0]?.user.id).not.toBe(first.items[0]?.user.id);
});

it('does not offer a service identity as a person to notify', () => {
  const world = new World(), ada = world.people.get('ada')!;
  const service = world.addPerson(me(30, 'ci-robot', 'CI robot', { kind: 'service' }), 'synthetic');
  world.access.handle(ada, 'PATCH', `/documents/${id(7, 3)}/permissions`, new URLSearchParams(), { name: [{ holders: [{ kind: 'user', id: service.me.id }], choice: 'full_view' }] });
  expect(world.documents.readable(service).some((doc) => doc.id === id(7, 3))).toBe(true);
  const result = world.peopleWork.handle(ada, 'GET', `/documents/${id(7, 3)}/mentions`, new URLSearchParams({ q: 'ci-robot' }), {})?.body as { items: { user: UserRef }[] };
  expect(result.items.some((candidate) => candidate.user.id === service.me.id)).toBe(false);
});

it('gives a group member access and sends one notice when the key is repeated', () => {
  const world = new World();
  const ada = world.people.get('ada')!, dora = world.people.get('dora')!, grace = world.people.get('grace')!, vera = world.people.get('vera')!;
  const query = new URLSearchParams();
  const chat = world.chats.handle(ada, 'POST', `/orgs/${ACME.org}/chats`, query, { user_ids: [dora.me.id, grace.me.id], name: 'Team' })?.body as Chat;
  const path = `/chats/${chat.id}/permissions/people/${vera.me.id}`;
  const before = world.peopleWork.handle(ada, 'GET', `/chats/${chat.id}/notify/people/${vera.me.id}`, query, {})?.body as { gives: string[]; can_view: boolean };
  expect(before.can_view).toBe(false);
  expect(before.gives).toEqual(['full_interact']);
  const body = { choice: 'full_interact', notify: true };
  expect(world.peopleWork.handle(ada, 'POST', path, query, body, 'same-act')?.status).toBe(204);
  expect(world.peopleWork.handle(ada, 'POST', path, query, body, 'same-act')?.status).toBe(204);
  const after = world.chats.handle(vera, 'GET', `/chats/${chat.id}`, query, {})?.body as Chat;
  expect(after.members?.some((member) => member.id === vera.me.id)).toBe(true);
  const inbox = world.notifications.control('/state', { user: vera.me.username })?.body as { rows: { kind: string }[] };
  expect(inbox.rows.filter((row) => row.kind === 'person.notify')).toHaveLength(1);
});

it('sends a single document notice per reader on a repeated explicit act', () => {
  const world = new World();
  const ada = world.people.get('ada')!, grace = world.people.get('grace')!;
  const document = id(7, 1), query = new URLSearchParams(), path = `/documents/${document}/notify`;
  const block = [...world.documents.docs.get(document)!.blocks.values()].find((one) => !one.removed)!;
  const body = { user_ids: [grace.me.id, grace.me.id], anchor: { block: block.id }, note: 'Please look' };
  expect(world.peopleWork.handle(ada, 'POST', path, query, body, 'notice-act')?.status).toBe(204);
  expect(world.peopleWork.handle(ada, 'POST', path, query, body, 'notice-act')?.status).toBe(204);
  const inbox = world.notifications.control('/state', { user: grace.me.username })?.body as { rows: { kind: string; body: string; url: string }[] };
  const notices = inbox.rows.filter((row) => row.kind === 'person.notify');
  expect(notices).toHaveLength(1);
  expect(notices[0]).toMatchObject({ body: 'Please look', url: `/doc/${document}?block=${block.id}` });
  expect(() => world.peopleWork.handle(ada, 'POST', path, query, { user_ids: [grace.me.id], note: 'a\u2028b' }, 'multiline')).toThrow(Refusal);
});

it('grants a document and notifies once when the same act is retried', () => {
  const world = new World(), ada = world.people.get('ada')!, grace = world.people.get('grace')!;
  const document = id(7, 3), path = `/documents/${document}/permissions`, query = new URLSearchParams();
  expect(world.documents.readable(grace).some((doc) => doc.id === document)).toBe(false);
  const body = { name: [{ holders: [{ kind: 'user', id: grace.me.id }], choice: 'full_view', export: false }], notify: true };
  expect(world.peopleWork.handle(ada, 'PATCH', path, query, body, 'grant-notice')?.status).toBe(200);
  expect(world.peopleWork.handle(ada, 'PATCH', path, query, body, 'grant-notice')?.status).toBe(200);
  expect(world.documents.readable(grace).some((doc) => doc.id === document)).toBe(true);
  const inbox = world.notifications.control('/state', { user: grace.me.username })?.body as { rows: { kind: string }[] };
  expect(inbox.rows.filter((row) => row.kind === 'person.notify')).toHaveLength(1);
});

it('refuses an opted-out grant and leaves the document private', () => {
  const world = new World(), ada = world.people.get('ada')!, grace = world.people.get('grace')!;
  const document = id(7, 3), path = `/documents/${document}/permissions`;
  world.notifications.prefs.set(grace.me.username, { revision: 1, pause_until: null, approvals_always: true, chat_direct_level: 'all', chat_node_level: 'mentions', quiet_hours: null, disabled_kinds: [noticeKind('person.notify')] });
  const body = { name: [{ holders: [{ kind: 'user', id: grace.me.id }], choice: 'full_view', export: false }], notify: true };
  expect(() => world.peopleWork.handle(ada, 'PATCH', path, new URLSearchParams(), body, 'refused-grant')).toThrow(Refusal);
  expect(world.documents.readable(grace).some((doc) => doc.id === document)).toBe(false);
});

it('projects the same stored notice actor for initial reads and live publication', () => {
  const world = new World();
  const ada = world.people.get('ada'), grace = world.people.get('grace');
  if (ada === undefined || grace === undefined) throw new Error('the known people are seeded');
  expect(world.notifications.control('/row', { user: 'ada', kind: 'chat.message', actor: 'grace', title: 'A message', parameters: { subject: 'Review' } })).toMatchObject({ status: 200, body: { actor: { id: grace.me.id } } });
  expect(world.notifications.handle(ada, 'GET', '/notifications', new URLSearchParams(), {})).toMatchObject({ status: 200, body: { items: [{ actor: { id: grace.me.id } }] } });
  expect(world.notifications.add('ada', { kind: 'doc.comment', title: 'A reply', data: { by: grace.me.id } })).toMatchObject({ actor: { id: grace.me.id } });
});
