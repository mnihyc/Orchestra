/** Native resource events carry the shown row; ordinary updates never repair its item or list over HTTP. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, resourceChanges, userEventChanges } from '@/api/acts';
import { missingInventoryProjection } from '@/api/cache';
import { resourceRows, type ResourceRows } from '@/api/listed';
import { setResourceFollowing } from '@/realtime/resourceFollowing';
import { inventoryRow } from '../doubles/everything-row';
import { detailHome, detailHost, detailUnitProvider } from '../component/resource-detail-fixtures';

it.each([['host', detailHost], ['provider', detailUnitProvider]] as const)('applies a typed %s event to its detail, list and known name row without a request', async (kind, native) => {
  const cache = new QueryClient();
  const before = { ...native, id: 'watched-resource', name: 'Before' };
  const after = { ...before, name: 'After', revision: before.revision + 1 };
  const detail = [kind, before.id], list = [kind === 'host' ? 'hosts' : 'providers'];
  const names = ['resource-navigation', 'reader', 'resources', { kinds: [kind], resource: { details: false } }];
  const named = inventoryRow({ id: before.id, kind, name: before.name, parent_id: 'resources', path: [detailHome] });
  const page = { pages: [{ items: [named], next_cursor: null }], pageParams: [null] };
  cache.setQueryData(detail, before); cache.setQueryData(list, resourceRows<typeof before>([before])); cache.setQueryData(names, page);
  const readDetail = vi.fn(() => Promise.resolve(after)), readList = vi.fn(() => Promise.resolve(resourceRows<typeof after>([after]))), readNames = vi.fn(() => Promise.resolve(page));
  const one = new QueryObserver(cache, { queryKey: detail, queryFn: readDetail, staleTime: Infinity });
  const many = new QueryObserver(cache, { queryKey: list, queryFn: readList, staleTime: Infinity });
  const branch = new QueryObserver(cache, { queryKey: names, queryFn: readNames, staleTime: Infinity });
  const stop = [one.subscribe(() => undefined), many.subscribe(() => undefined), branch.subscribe(() => undefined)];
  setResourceFollowing(kind, before.id, true);
  try {
    const changes = resourceChanges(kind, before.id, { what: 'changed', row: { kind, value: after } });
    expect(changes).toEqual([{ row: { kind, id: before.id, value: after, insert: true } }]);
    // The general notice may arrive first; it carries no new row and must not race a repair GET.
    await applyChanges(userEventChanges('resources', { kind, id: before.id }), cache);
    expect(cache.getQueryData(detail)).toEqual(before);
    expect(readDetail).not.toHaveBeenCalled(); expect(readList).not.toHaveBeenCalled(); expect(readNames).not.toHaveBeenCalled();
    await applyChanges(changes, cache);
    expect(cache.getQueryData(detail)).toEqual(after);
    const rows = cache.getQueryData<ResourceRows<typeof before>>(list);
    expect(rows?.[0]).toEqual(after); expect(rows?.privateRows).toEqual([]);
    expect(cache.getQueryData(names)).toEqual({ ...page, pages: [{ ...page.pages[0], items: [{ ...named, name: 'After' }] }] });
    expect(readDetail).not.toHaveBeenCalled(); expect(readList).not.toHaveBeenCalled(); expect(readNames).not.toHaveBeenCalled();
  } finally { setResourceFollowing(kind, before.id, false); for (const off of stop) off(); cache.clear(); }
});

it('retains exact native inventory repair for changed authority, private paths, and the pre-fetch cut of a legacy repair', () => {
  const cache = new QueryClient(), id = 'watched-resource';
  const before = { ...detailHost, id }, after = { ...before, access: { ...before.access, permissions: ['host.view'] } };
  const key = ['resource-navigation', 'reader', 'resources', { kinds: ['host'], resource: { details: false } }];
  const named = inventoryRow({ id, kind: 'host', name: before.name });
  const page = { pages: [{ items: [named], next_cursor: null }], pageParams: [null] };
  const read = vi.fn(() => Promise.resolve(page)); cache.setQueryData(key, page); cache.setQueryData(['host', id], before);
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }), off = observer.subscribe(() => undefined);
  try {
    expect(missingInventoryProjection(cache, { kind: 'host', id, value: after, insert: true })).toEqual(key);
    // A legacy GET has installed its answer already; compare with what was held before that GET.
    cache.setQueryData(['host', id], after);
    expect(missingInventoryProjection(cache, { kind: 'host', id, value: after, insert: true }, { row: before })).toEqual(key);
    cache.setQueryData(key, { ...page, pages: [{ ...page.pages[0], items: [{ ...named, name: null, my_access: 'path_only' }] }] });
    expect(missingInventoryProjection(cache, { kind: 'host', id, value: after, insert: true })).toEqual(key);
    expect(read).not.toHaveBeenCalled();
  } finally { off(); cache.clear(); }
});

it('rejects a different resource kind in the typed row envelope', () => {
  expect(resourceChanges('host', detailHost.id, { what: 'changed', row: { kind: 'provider', value: detailUnitProvider } })).toEqual([]);
});

it('clears a revoked rich view without a GET while retaining independently admitted sign-in progress and names', async () => {
  const cache = new QueryClient(), id = detailUnitProvider.id;
  const key = ['provider', id], progressKey = ['provider-login', id];
  const progress = { state: 'awaiting_user', method: 'login', shows: [], asks: null, expires_at: null, error: null };
  const bound = { provider: { id, name: detailUnitProvider.name } };
  cache.setQueryData(key, detailUnitProvider); cache.setQueryData(progressKey, progress); cache.setQueryData(['binding-reference'], bound);
  const read = vi.fn(() => Promise.resolve(detailUnitProvider));
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity }), off = observer.subscribe(() => undefined);
  try {
    await applyChanges(resourceChanges('provider', id, { what: 'changed', view_revoked: true }), cache);
    expect(cache.getQueryData(key)).toBeUndefined(); expect(observer.getCurrentResult().isError).toBe(true);
    expect(cache.getQueryData(progressKey)).toEqual(progress); expect(cache.getQueryData(['binding-reference'])).toEqual(bound);
    expect(read).not.toHaveBeenCalled();
  } finally { off(); cache.clear(); }
});
