/** A home choice uses its native root, never a department id or an inferred owner. */
import { beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { findAct } from '@/api/acts';
import { moveResourceHome } from '@/api/resourceHome';
import { groupHomeMoveEntries, homeMoveEntries, homeMoveReason, moveRoot, type HomeMoveKind } from '@/features/duplicate/move';
import { useDuplicateHost } from '@/features/duplicate/target';
import { i18n } from '@/i18n';

const calls = vi.hoisted(() => ({ json: vi.fn<(path: string, options: unknown) => Promise<unknown>>(), patch: vi.fn<(path: string, options: unknown) => Promise<{ data: unknown; response: Response }>>() }));
vi.mock('@/api/trace', () => ({ traceJson: calls.json }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, PATCH: calls.patch } }; });
beforeEach(() => { vi.clearAllMocks(); useDuplicateHost.getState().close(); });
const target = { kind: 'provider', id: 'provider', name: 'Provider', rootOrgId: 'root', revision: 7 } as const;
const place = { id: 'department', kind: 'org', color: null, folder: false, path: 'Team / Department', rootOrgId: 'root' } as const;
it('accepts a department of the same root even when its own id differs', () => {
  expect(homeMoveReason(target, place, i18n.t)).toBe(false);
  expect(homeMoveReason(target, { ...place, rootOrgId: 'different-root' }, i18n.t)).toBe(i18n.t('duplicate.moveOtherOrgUnavailable'));
  const { rootOrgId: _root, ...unknownRoot } = place;
  expect(homeMoveReason(target, unknownRoot, i18n.t)).toBe(i18n.t('duplicate.homeUnknown'));
  expect(homeMoveReason(target, { ...place, kind: 'topic' }, i18n.t)).toBe(i18n.t('duplicate.homeRequired'));
});
it('uses the declared resource root or the supplied native machine path, with no home-id guess', () => {
  expect(moveRoot({ org_id: 'root' })).toBe('root');
  expect(moveRoot({ home_path: [{ id: 'root', kind: 'org', name: null, redacted: true }, { id: 'department', kind: 'org', name: 'Dept', redacted: false }] })).toBe('root');
  expect(moveRoot({ org_id: null, home_path: [] })).toBeUndefined();
});
it('Manage permits a same-root move independently of View or first-owner identity', () => {
  const row = { id: 'provider', name: 'Provider', org_id: 'root', access: { permissions: ['provider.manage'], administers: false, through: [] } };
  const entries = homeMoveEntries(i18n.t, 'provider', row);
  expect(entries[0]).toMatchObject({ id: 'move-home', disabled: false });
  const entry = entries[0]; if (entry !== undefined && 'onSelect' in entry) entry.onSelect();
  expect(useDuplicateHost.getState().current).toMatchObject({ mode: 'move', target: { id: row.id, rootOrgId: 'root' } });
  expect(homeMoveEntries(i18n.t, 'provider', { ...row, access: { ...row.access, permissions: ['provider.view'] } })[0]).toMatchObject({ disabled: i18n.t('duplicate.manageRequired') });
});
it('offers only home Move for machine and host, without a Duplicate act', () => {
  for (const kind of ['machine', 'host'] as const) {
    const entries = homeMoveEntries(i18n.t, kind, { id: kind, name: kind, org_id: 'root', access: { permissions: [`${kind}.manage`], administers: false, through: [] } });
    expect(entries.map((entry) => entry.id)).toEqual(['move-home']);
  }
});
it('uses native group management and leaves direct chats without Move', () => {
  expect(groupHomeMoveEntries(i18n.t, { id: 'group', name: 'Group', kind: 'group', org_id: 'root', can: { manage: true } })[0]).toMatchObject({ disabled: false });
  expect(groupHomeMoveEntries(i18n.t, { id: 'direct', name: null, kind: 'direct', org_id: 'root', can: { manage: true } })).toEqual([]);
  expect(groupHomeMoveEntries(i18n.t, { id: 'group', name: 'Group', kind: 'group', org_id: 'root', can: { manage: false } })[0]).toMatchObject({ disabled: i18n.t('duplicate.manageRequired') });
});
const families: Exclude<HomeMoveKind, 'chat'>[] = ['provider', 'agent', 'template', 'mcp_server', 'mount', 'skill', 'environment', 'host', 'machine'];
it.each(families)('moves %s with its declared native home writer and stable identity', async (kind) => {
  const object = kind === 'mcp_server' ? 'mcp-servers' : `${kind}s`;
  calls.json.mockResolvedValue({ projection: 'receipt', kind, object_id: 'source', home_id: 'department', changed: true });
  expect(await moveResourceHome(kind, 'source', { home_id: 'department', expected_revision: 7 }, 'one-move')).toMatchObject({ object_id: 'source', home_id: 'department' });
  expect(calls.json).toHaveBeenCalledExactlyOnceWith(`/${object}/source/home`, { method: 'PUT', body: { home_id: 'department', expected_revision: 7 }, key: 'one-move' });
  expect(findAct('PUT', `/${object}/source/home`)).not.toBeNull();
});
it('group Move remains ChatUpdate and sends no copied name, visibility or roster', async () => {
  calls.patch.mockResolvedValue({ data: { projection: 'receipt', chat_id: 'group' }, response: new Response(null, { status: 200 }) });
  expect(await moveResourceHome('chat', 'group', { home_id: 'department' }, 'group-move')).toEqual({ chat_id: 'group' });
  expect(calls.patch).toHaveBeenCalledWith('/chats/{id}', expect.objectContaining({ params: { path: { id: 'group' } }, body: { home_id: 'department' } }));
  expect(calls.json).not.toHaveBeenCalled();
});
it('does not accept another identity in a home receipt', async () => {
  calls.json.mockResolvedValue({ projection: 'receipt', kind: 'provider', object_id: 'foreign', home_id: 'department', changed: true });
  await expect(moveResourceHome('provider', 'source', { home_id: 'department' }, 'one-move')).rejects.toThrow('did not identify');
});
