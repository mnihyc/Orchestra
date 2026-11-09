/** Administrative operations retain their exact account/credential identity and personal custody. */
import { QueryClient, InfiniteQueryObserver, QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { applyChanges, findAct } from '@/api/acts';
import { adminSessionsQuery, adminTokenActivityQuery, adminTokensQuery, deleteOwnUpload, environmentSnapshotsQuery, ownUploadsQuery, reachKeys, refreshReach, revokeAdminSession, revokeAdminToken } from '@/api/adminReach';
import { LiveQueryClient } from '@/api/query';
import { inventoryFile } from '@/features/uploads/Inventory';
import { reachPerson, reachToken, reachUpload } from '../doubles/admin-reach';
import type { HttpTransport } from '../doubles/http-wire';

const calls = vi.hoisted(() => ({ get: vi.fn<HttpTransport>(), remove: vi.fn<HttpTransport>() }));
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: calls.get, DELETE: calls.remove } }; });
const caches: QueryClient[] = [];
const before = client.session.getState();
const cache = () => { const value = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(value); return value; };
beforeEach(() => {
  calls.get.mockReset().mockResolvedValue({ data: { items: [], next_cursor: null }, response: new Response(null, { status: 200 }) });
  calls.remove.mockReset().mockResolvedValue({ response: new Response(null, { status: 204 }) });
});
afterEach(() => { caches.splice(0).forEach((item) => item.clear()); client.session.setState(before); });

it('keeps system tokens, a person’s tokens, sessions and retained activity as distinct native operations', async () => {
  const c = cache();
  await c.infiniteQuery(adminTokensQuery());
  await c.infiniteQuery(adminTokensQuery(reachPerson.id));
  await c.infiniteQuery(adminSessionsQuery(reachPerson.id));
  await c.query(adminTokenActivityQuery(reachPerson.id, reachToken().info.id, { limit: 12, cursor: 'opaque|activity' }));
  expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/admin/tokens', '/admin/users/{id}/tokens', '/admin/users/{id}/sessions', '/admin/users/{id}/tokens/{token_id}/activity']);
  expect(calls.get.mock.calls[3]?.[1].params).toEqual({ path: { id: reachPerson.id, token_id: reachToken().info.id }, query: { limit: 12, cursor: 'opaque|activity' } });
  expect(adminTokensQuery().meta).toEqual({ requiresAdminMode: true, requiresSystemAdmin: true });
  expect(environmentSnapshotsQuery('environment', true).queryKey).not.toEqual(environmentSnapshotsQuery('environment', false).queryKey);
});

it('passes the server continuation unchanged and does not fetch it until More', async () => {
  calls.get.mockResolvedValueOnce({ data: { items: [reachToken()], next_cursor: 'opaque|tokens|person' }, response: new Response(null, { status: 200 }) });
  const observer = new InfiniteQueryObserver(cache(), { ...adminTokensQuery(reachPerson.id), enabled: false });
  const unsubscribe = observer.subscribe(() => undefined);
  try {
    await observer.refetch(); expect(calls.get).toHaveBeenCalledOnce();
    await observer.fetchNextPage();
    expect(calls.get.mock.calls[1]?.[1].params).toEqual({ path: { id: reachPerson.id }, query: { limit: 50, cursor: 'opaque|tokens|person', owner_id: null } });
  } finally { unsubscribe(); observer.destroy(); }
});

it('revokes exactly one target, and ALL is one native act including the requesting session', async () => {
  await revokeAdminToken('key'); await revokeAdminToken('key', 'person');
  await revokeAdminSession('person', 'session'); await revokeAdminSession('person');
  expect(calls.remove.mock.calls).toEqual([
    ['/admin/tokens/{id}', { params: { path: { id: 'key' } } }],
    ['/admin/users/{id}/tokens/{token_id}', { params: { path: { id: 'person', token_id: 'key' } } }],
    ['/admin/users/{id}/sessions/{session_id}', { params: { path: { id: 'person', session_id: 'session' } } }],
    ['/admin/users/{id}/sessions', { params: { path: { id: 'person' } } }],
  ]);
  expect(calls.get).not.toHaveBeenCalled();
});

it('uses native own-upload cancellation/release targets and never a machine path', async () => {
  await cache().infiniteQuery(ownUploadsQuery());
  expect(calls.get.mock.calls[0]?.[0]).toBe('/me/uploads');
  await deleteOwnUpload(reachUpload({ kind: 'session', session_id: 'actual-session', blob: null, state: 'partial' }));
  await deleteOwnUpload(reachUpload());
  expect(calls.remove.mock.calls.map(([path, request]) => [path, request.params?.path])).toEqual([
    ['/uploads/sessions/{upload_id}', { upload_id: 'actual-session' }], ['/uploads/blobs/{hash}', { hash: 'a'.repeat(64) }],
  ]);
  await expect(deleteOwnUpload(reachUpload({ kind: 'machine', can_delete: false }))).rejects.toThrow();
  expect(calls.remove).toHaveBeenCalledTimes(2);
});

it('opens completed admin bytes by custody id, never by the hash or opaque inventory key', () => {
  const row = reachUpload({ id: 'opaque:inventory-key', can_open: true });
  expect(inventoryFile(row, 'notes.txt', true)).toMatchObject({ url: `/admin/uploads/${row.content_id}/content?name=notes.txt`, adminOnly: true });
  expect(inventoryFile(row, 'notes.txt', false)?.url).toBe(`/uploads/blobs/${'a'.repeat(64)}?name=notes.txt`);
  // can_open is administrative custody; an ordinary owner still has their own blob operation.
  expect(inventoryFile(reachUpload({ can_open: false }), 'notes.txt', false)?.url).toBe(`/uploads/blobs/${'a'.repeat(64)}?name=notes.txt`);
  for (const variant of [reachUpload({ state: 'partial' }), reachUpload({ state: 'expired' }), reachUpload({ can_open: false }), reachUpload({ content_id: null })]) {
    expect(inventoryFile(variant, 'notes.txt', true)).toBeNull();
  }
});

it('rejects a late personal inventory answer after the requesting reader changes', async () => {
  client.session.setState({ userId: 'first', readerVersion: 1 });
  calls.get.mockImplementation(() => { client.session.setState({ userId: 'second', readerVersion: 2 }); return Promise.resolve({ data: { items: [reachUpload()], next_cursor: null }, response: new Response(null, { status: 200 }) }); });
  await expect(cache().infiniteQuery(ownUploadsQuery())).rejects.toMatchObject({ name: 'AbortError' });
});

it('refreshes only existing exact feature suppliers under the live client’s broad-invalidation guard', async () => {
  const c = new LiveQueryClient(); caches.push(c);
  const own = [...reachKeys.uploads, false, ['reader', 1]], other = ['unrelated'];
  c.setQueryData(own, { pages: [], pageParams: [] }); c.setQueryData(other, ['kept']);
  await refreshReach(c, reachKeys.uploads);
  expect(c.getQueryState(own)?.isInvalidated).toBe(true);
  expect(c.getQueryState(other)?.isInvalidated).toBe(false);
});

it.each([
  '/admin/tokens/key', '/admin/users/person/tokens/key',
  '/admin/users/person/sessions', '/admin/users/person/sessions/session',
])('publishes the accepted %s through its exact visible inventory suppliers once', async (path) => {
  const c = new LiveQueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } }); caches.push(c);
  // Markers measure the registry's actual supplier selection, independent of the native DTO body.
  const token = path.includes('/tokens/');
  const keys = token ? [adminTokensQuery().queryKey, adminTokensQuery('person').queryKey]
    : [adminSessionsQuery('person').queryKey];
  const read = keys.map(() => vi.fn(() => Promise.resolve(['accepted'])));
  const stops = keys.map((tagged, index) => {
    const key = [...tagged], queryFn = read[index]; if (queryFn === undefined) throw new Error('Missing inventory supplier');
    c.setQueryData(key, ['before']);
    return new QueryObserver(c, { queryKey: key, queryFn, staleTime: Infinity }).subscribe(() => undefined);
  });
  const unrelatedKey = token ? [...adminSessionsQuery('person').queryKey] : [...adminSessionsQuery('someone-else').queryKey];
  const unrelated = vi.fn(() => Promise.resolve(['unrelated'])); c.setQueryData(unrelatedKey, ['unrelated']);
  stops.push(new QueryObserver(c, { queryKey: unrelatedKey, queryFn: unrelated, staleTime: Infinity }).subscribe(() => undefined));
  const activityKey = ['admin-credentials', 'activity', 'person', 'key']; c.setQueryData(activityKey, ['retained history']);
  try {
    const accepted = findAct('DELETE', path); expect(accepted).not.toBeNull();
    await applyChanges(accepted?.changes ?? [], c);
    keys.forEach((key, index) => { expect(read[index]).toHaveBeenCalledOnce(); expect(c.getQueryData(key)).toEqual(['accepted']); });
    expect(unrelated).not.toHaveBeenCalled(); expect(c.getQueryData(activityKey)).toEqual(['retained history']);
  } finally { stops.forEach((stop) => stop()); }
});
