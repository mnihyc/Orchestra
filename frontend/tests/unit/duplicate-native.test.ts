/** Copies carry identities and native choices, never source bodies, secrets or privilege recipes. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import type * as Client from '@/api/client';
import { beforeEach, expect, it, vi } from 'vitest';
import { duplicate, duplicateBody, duplicateObjects, duplicatePath, previewChatDuplicate, type DuplicateKind } from '@/api/duplicates';
import { applyChanges, findAct } from '@/api/acts';
import { i18n } from '@/i18n';
import { duplicateEntries, openDuplicate, resourceDuplicateEntries, useDuplicateHost } from '@/features/duplicate/target';
import type { ResourceAccess } from '@/generated/api-types';

const calls = vi.hoisted(() => ({ json: vi.fn<(path: string, options?: unknown) => Promise<unknown>>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api,
  POST: (path: string, options: { params: { path: { id: string } }; body?: unknown; headers?: HeadersInit; signal?: AbortSignal }) => {
    const key = new Headers(options.headers).get('Idempotency-Key');
    return calls.json(path.replace('{id}', encodeURIComponent(options.params.path.id)), { method: 'POST', ...(options.body === undefined ? {} : { body: options.body }), ...(key === null ? {} : { key }), ...(options.signal === undefined ? {} : { signal: options.signal }) })
      .then((data) => ({ data, response: new Response(null, { status: 200 }) }));
  },
} }; });
beforeEach(() => { vi.clearAllMocks(); useDuplicateHost.getState().close(); });
const kinds = Object.keys(duplicateObjects) as DuplicateKind[];

it.each(kinds)('keeps %s duplication in its declared native operation and accepts the sparse identity', async (kind) => {
  const receipt = { projection: 'receipt', kind, id: 'fresh-resource' } as const;
  calls.json.mockResolvedValue(receipt);
  expect(await duplicate(kind, 'original', { destination: { kind: 'node', id: 'another-org' }, expected_revision: 7 }, 'one-act')).toEqual(receipt);
  const ordinary = { name: null, destination: { kind: 'node', id: 'another-org' }, expected_revision: 7 };
  const expected = kind === 'chat' ? { copy: ordinary, notify: null, notify_preview_id: null } : kind === 'mcp_server' ? { ...ordinary, machine_id: null } : kind === 'environment' ? { ...ordinary, managed: null, machine_id: null } : ordinary;
  expect(calls.json).toHaveBeenCalledExactlyOnceWith(`/${duplicateObjects[kind]}/original/duplicate`, {
    method: 'POST', key: 'one-act', body: expected,
  });
});
it('leaves native naming and same-place placement unspecified for the one-click act', async () => {
  calls.json.mockResolvedValue({ projection: 'receipt', kind: 'provider', id: 'new-provider' });
  await duplicate('provider', 'source', {}, 'one-act');
  expect(calls.json).toHaveBeenCalledWith('/providers/source/duplicate', { method: 'POST', key: 'one-act', body: { name: null, destination: null, expected_revision: null } });
});
it('names the draft-reading operation only for an explicit skill draft', () => {
  expect(duplicatePath('skill', 'source', true)).toBe('/skills/source/draft/duplicate');
  expect(duplicatePath('skill', 'source')).toBe('/skills/source/duplicate');
  expect(duplicatePath('provider', 'source', true)).toBe('/providers/source/duplicate');
});
it('keeps a physical MCP recipient independent of home, and ignores that field for other families', () => {
  const body = { destination: { kind: 'node', id: 'destination' } } as const;
  expect(duplicateBody('mcp_server', body, { machineId: 'usable-machine' })).toEqual({ ...body, name: null, expected_revision: null, machine_id: 'usable-machine' });
  expect(duplicateBody('provider', body, { machineId: 'usable-machine' })).toEqual({ ...body, name: null, expected_revision: null });
});
it('binds an explicit group notification to its own native preview', async () => {
  calls.json.mockResolvedValue({ people: 3, notify_limit: 200, preview_id: 'preview' });
  await previewChatDuplicate('group');
  expect(calls.json).toHaveBeenCalledWith('/chats/group/duplicate/preview', { method: 'POST' });
  expect(duplicateBody('chat', {}, { notify: { preview: 'preview' } })).toEqual({ copy: { name: null, destination: null, expected_revision: null }, notify: true, notify_preview_id: 'preview' });
  expect(duplicateBody('chat', {})).toEqual({ copy: { name: null, destination: null, expected_revision: null }, notify: null, notify_preview_id: null });
});
it('rejects an acknowledgement for a different family without opening it', async () => {
  calls.json.mockResolvedValue({ projection: 'receipt', kind: 'environment', id: 'foreign' });
  await expect(duplicate('provider', 'source', {}, 'one-act')).rejects.toThrow('did not identify');
});
it.each(kinds)('registers %s creation without marking its source content changed', (kind) => {
  const act = findAct('POST', `/${duplicateObjects[kind]}/source/duplicate`, { answer: { projection: 'receipt', kind, id: 'new-copy' } });
  expect(act).not.toBeNull();
  const missing = act?.changes.flatMap((change) => 'missing' in change ? change.missing : []) ?? [];
  expect(missing.some((view) => view.key[0] === 'everything' && view.id === undefined)).toBe(true);
  expect(missing.some((view) => view.id === 'source')).toBe(false);
  expect(act?.changes.some((change) => 'row' in change && change.row.id === 'source')).toBe(false);
});
it('registers draft creation and a read-only audience preview separately', () => {
  expect(findAct('POST', '/skills/source/draft/duplicate')).not.toBeNull();
  expect(findAct('POST', '/chats/source/duplicate/preview')?.changes).toEqual([]);
});
it('does not turn a permission-page name or Manage into readable source configuration', () => {
  const access: ResourceAccess = { administers: false, permissions: ['provider.manage'], through: [] };
  const entries = resourceDuplicateEntries(i18n.t, 'provider', { id: 'source', name: 'Named provider', access });
  expect(entries).toHaveLength(2);
  for (const entry of entries) {
    expect('disabled' in entry && entry.disabled).toBe(i18n.t('duplicate.readRequired'));
    if ('onSelect' in entry) entry.onSelect();
  }
  expect(useDuplicateHost.getState().current).toBeNull();
});
it('requires the actual environment files row and the actual skill draft row', () => {
  const row = (permissions: string[]) => ({ id: 'source', name: 'Source', access: { permissions, administers: false, through: [] } });
  expect(resourceDuplicateEntries(i18n.t, 'environment', row(['environment.view']))[0]).toMatchObject({ disabled: i18n.t('duplicate.filesRequired') });
  expect(resourceDuplicateEntries(i18n.t, 'environment', row(['environment.view', 'files.view']))[0]).toMatchObject({ disabled: false });
  expect(resourceDuplicateEntries(i18n.t, 'skill', row(['skill.view']), { draft: true })[0]).toMatchObject({ disabled: i18n.t('duplicate.readRequired') });
  expect(resourceDuplicateEntries(i18n.t, 'skill', row(['skill.manage']), { draft: true })[0]).toMatchObject({ disabled: false });
});
it('uses the environment parent’s actual View for environment-owned MCP definitions', () => {
  expect(resourceDuplicateEntries(i18n.t, 'mcp_server', { id: 'source', name: 'Tools', access: { permissions: ['environment.view'], administers: false, through: [] } }, { sourceView: 'environment.view' })[0]).toMatchObject({ disabled: false });
});
it('one open copy cannot be replaced by a second press with another act identity', () => {
  const target = { kind: 'provider', id: 'source', name: 'Provider' } as const;
  openDuplicate(target, true); const before = useDuplicateHost.getState().current;
  openDuplicate(target, true);
  expect(useDuplicateHost.getState().current).toBe(before);
  expect(duplicateEntries(i18n.t, target).map((entry) => entry.id)).toEqual(['duplicate', 'duplicate-to']);
});

it('a content-free copy receipt repairs its visible inventory without touching the source or inventing a row', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const inventory = ['everything', { kinds: ['provider'] }] as const, source = ['provider', 'source'] as const;
  const before = { items: [] as { id: string; name: string }[] }, after = { items: [{ id: 'new-copy', name: 'Native readable copy' }] }, original = { id: 'source', name: 'Original' };
  const readInventory = vi.fn(() => Promise.resolve(after)), readSource = vi.fn(() => Promise.resolve(original));
  cache.setQueryData(inventory, before); cache.setQueryData(source, original);
  const stops = [new QueryObserver(cache, { queryKey: inventory, queryFn: readInventory, staleTime: Infinity }).subscribe(() => undefined),
    new QueryObserver(cache, { queryKey: source, queryFn: readSource, staleTime: Infinity }).subscribe(() => undefined)];
  try {
    await applyChanges(findAct('POST', '/providers/source/duplicate', { answer: { projection: 'receipt', kind: 'provider', id: 'new-copy' } })?.changes ?? [], cache);
    expect(cache.getQueryData(inventory)).toEqual(after); expect(readInventory).toHaveBeenCalledOnce();
    expect(cache.getQueryData(source)).toEqual(original); expect(readSource).not.toHaveBeenCalled();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});
