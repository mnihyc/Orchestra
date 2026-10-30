// @vitest-environment node
/** The existing restore endpoint returns the state it stored, as native node_restore does after commit. */
import { expect, it } from 'vitest';
import type { AuthResult, Org, OrgPage } from '../../src/generated/api-types.ts';
import { World } from '../doubles/world.ts';

it('returns the restored archived org and moves it from deleted to live native projections', () => {
  const world = new World(), login = world.login({ login: 'ada', password: 'correct horse battery' });
  const bearer = `Bearer ${(login.body as AuthResult).access_token}`; world.setAdminMode(bearer, { as_admin: true });
  const actor = world.browserPerson(bearer), deleted = world.admin.deletedOrgs[0];
  if (deleted === undefined) throw new Error('The deleted org fixture is missing');
  const reply = world.admin.handle(actor, 'POST', `/nodes/${deleted.id}/restore`, new URLSearchParams(), {});
  expect(reply?.status).toBe(200); const restored = reply?.body as Org;
  expect(restored).toMatchObject({ id: deleted.id, archived: true }); expect(restored.archived_at).not.toBeNull();
  const live = world.admin.handle(actor, 'GET', '/admin/orgs', new URLSearchParams({ q: deleted.name }), {})?.body as OrgPage;
  const gone = world.admin.handle(actor, 'GET', '/admin/orgs', new URLSearchParams({ deleted: 'only', q: deleted.name }), {})?.body as OrgPage;
  expect(live.items).toEqual([restored]); expect(gone.items).toEqual([]);
});
