/** Full live pages for the general audience and local inherited-row edits. */
import { indexCatalogue } from '@/features/access/model';
import type { AccessKind } from '@/api/access';
import type { Entry, Holder, NodeRef, PermissionCatalogueGetReply, PermissionPage, Role, UserRef } from '@/generated/api-types';

import data from '../doubles/catalogue.json';

export const CATALOGUE = indexCatalogue(data as unknown as PermissionCatalogueGetReply);
export const TOPIC: NodeRef = { id: '00000000-0000-4000-8002-000000000001', kind: 'topic', name: 'Fix login', redacted: false };
export const ADA: UserRef = { id: '00000000-0000-4000-8001-000000000001', kind: 'human', username: 'ada', display_name: 'Ada', avatar: { text: 'A', color: 1 } };
export const BEA: UserRef = { ...ADA, id: '00000000-0000-4000-8001-000000000002', username: 'bea', display_name: 'Bea' };
export const role = (name: Role): Holder => ({ kind: 'role', role: name, at: null, count: 2 });
export const generalRole = (): Holder => ({ kind: 'role', role: 'viewer', at: TOPIC, count: 4 });
export const named = (holder: Holder, n: number, more: Partial<Entry> = {}): Entry => ({
  id: `00000000-0000-4000-8009-${n.toString().padStart(12, '0')}`, holder, options: {}, outside: false, read_first: false,
  expires_at: null, note: '', source: 'page', created_by: ADA, created_at: '2026-10-01T09:00:00Z', revision: 1, ...more,
});

export function pageFor(kind: AccessKind = 'thread'): PermissionPage {
  const rows = CATALOGUE.of(kind).filter((def) => def.name !== 'access.own').map((def) => {
    const level: Role | null = CATALOGUE.choice('full_manage', kind).includes(def.name) ? 'developer'
      : CATALOGUE.choice('full_interact', kind).includes(def.name) ? 'interactor'
      : CATALOGUE.choice('full_view', kind).includes(def.name) ? 'viewer' : null;
    return { permission: def.name, profile: 'any' as const, inherit: true, inherited: { holders: level === null ? [] : [role(level)], from: TOPIC }, entries: [], locked: null, writable: true, can_give: true };
  });
  return {
    object: { id: '00000000-0000-4000-8003-000000000001', kind, name: 'Example' }, page: 'live', owner: ADA, owners: [named({ kind: 'user', user: ADA }, 1)],
    owners_writable: true, external: true, admins: { count: 1, of: [TOPIC, 'system'] }, rows, state: 'follows', followers: null,
    gives: { choices: ['full_view', 'full_interact', 'full_manage'], roles: [], agents: ['view', 'edit'], holders_share: true }, covers: null, depends: [], reaches: null, revision: 7,
  };
}
