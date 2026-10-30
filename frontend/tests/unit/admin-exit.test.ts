/** Ending administrator reach follows ordinary View provenance and the real nearest readable path. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { meKey } from '@/api/auth';
import { nodeKey, orgsKey } from '@/api/nodes';
import { documentKey } from '@/api/documents';
import { adminExitPlan, ordinaryDestination, ordinaryView, type AdminExitPlan } from '@/shell/adminExit';
import { me, org, personalHomeOf, userRef } from '../doubles/fixtures';

const person = me(1, 'ada', 'Ada');
const root = { id: 'root', kind: 'org', name: 'Acme', redacted: false } as const;
const folder = { id: 'folder', kind: 'project', name: 'Web', redacted: false } as const;
const privateProject = { id: 'private', kind: 'project', name: 'Private', redacted: false } as const;
const ordinary = { permissions: ['project.view'], admin_only: ['project.manage'], admin_place: 'system' };
const native = { permissions: ['project.view'], admin_access: { place: 'system' }, admin_only: ['project.view'] };
const clients: QueryClient[] = [];
function setup() {
  const cache = new QueryClient(); clients.push(cache);
  cache.setQueryData(meKey, person);
  cache.setQueryData(orgsKey, [org(personalHomeOf(person), 'ada', 'Ada', userRef(person), { org_kind: 'personal' })]);
  cache.setQueryData(nodeKey(root.id), { ...root, path: [root], access: { permissions: ['org.view'] } });
  cache.setQueryData(nodeKey(folder.id), { ...folder, path: [root, folder], access: ordinary });
  cache.setQueryData(nodeKey(privateProject.id), { ...privateProject, path: [root, folder, privateProject], access: native });
  return cache;
}
afterEach(() => { for (const cache of clients.splice(0)) cache.clear(); });

it('does not confuse administrative actions or a Private flag with needing native reach to view', () => {
  expect(ordinaryView({ private: true, access: ordinary }, 'project.view')).toBe('yes');
  expect(ordinaryView({ access: native }, 'project.view')).toBe('no');
  expect(ordinaryView(privateProject, 'project.view')).toBe('unknown');
});

it('returns a private project to its nearest ordinary folder without any extra read', async () => {
  const cache = setup();
  const ask = vi.fn<(place: unknown) => Promise<boolean>>();
  const plan = adminExitPlan(cache, '/project/private');
  expect(plan.parents.map((part) => part.id)).toEqual(['folder', 'root']);
  expect(await ordinaryDestination(plan, ask)).toBe('/project/folder');
  expect(ask).not.toHaveBeenCalled();
});

it('skips native-only ancestors and probes only an unknown parent, with its exact ordinary-session read', async () => {
  const cache = setup();
  cache.setQueryData(nodeKey(folder.id), { ...folder, access: native });
  cache.removeQueries({ queryKey: nodeKey(root.id), exact: true });
  const ask = vi.fn(() => Promise.resolve(true));
  expect(await ordinaryDestination(adminExitPlan(cache, '/project/private'), ask)).toBe('/org/root');
  expect(ask).toHaveBeenCalledExactlyOnceWith({ id: 'root', kind: 'node', href: '/org/root', access: 'unknown' });
});

it('leaves a document through its actual containing node, and a resource through its kind list', async () => {
  const cache = setup();
  cache.setQueryData(documentKey('doc'), { id: 'doc', node: folder, access: { ...native, admin_only: ['doc.view'] } });
  expect(await ordinaryDestination(adminExitPlan(cache, '/doc/doc'), () => Promise.resolve(false))).toBe('/project/folder');
  cache.setQueryData(['provider', 'private-provider'], { id: 'private-provider', access: native });
  expect(await ordinaryDestination(adminExitPlan(cache, '/resources/providers/private-provider'), () => Promise.resolve(false))).toBe('/resources/providers');
});

it('stays on an ordinarily readable page and falls back to the person’s own home from Management', async () => {
  const cache = setup();
  expect(await ordinaryDestination(adminExitPlan(cache, '/project/folder'), () => Promise.resolve(false))).toBeNull();
  expect(await ordinaryDestination(adminExitPlan(cache, '/admin/overview'), () => Promise.resolve(false))).toBe('/org/ada');
  const plan: AdminExitPlan = { origin: '/admin/overview', person: person.id, current: { href: '/admin/overview', access: 'no' }, parents: [], fallback: '/settings/profile' };
  expect(await ordinaryDestination(plan, () => Promise.resolve(false))).toBe('/settings/profile');
});
