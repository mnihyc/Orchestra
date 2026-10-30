/** Current metadata must replace old reads without resurrecting another page's refused names. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, client } from '@/api/client';
import type * as ClientModule from '@/api/client';
import { documentDirectoryQuery, type DocumentDirectoryRequest } from '@/api/documentTree';
import { adoptDocumentDirectory, documentDirectoryKey, retireDocumentDirectory } from '@/api/documentTreeCache';
import type { DocumentTreePage } from '@/generated/api-types';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: unknown) => Promise<DocumentTreePage>>() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: async (request: Promise<unknown>) => request };
});
const request: DocumentDirectoryRequest = { node: 'node-a', at: { kind: 'container', id: 'folder' } };
const snapshot = (title: string, node = 'node-a'): DocumentTreePage => ({
  items: [{ entry: { item: { kind: 'container', id: title }, parent: request.at, node, root: 'org', basename: title, title,
    revision: 1, owner: 'owner', deleted_at: null, deletion: null, files: 0, versions: 0 }, standing: { read: true, write: false, manage: false } }],
  next_cursor: 'next', feed: 'opaque-directory-feed', ancestors: [],
  directory: { at: request.at, node: { id: node, kind: 'project', name: null, redacted: true }, path: [], folder: null,
    ancestors: [], can_create: false, inspection: null, membership_seq: 1 },
});
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
let cache: QueryClient;
let reader: number | undefined;
beforeEach(() => {
  cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  reader = client.session.getState().readerVersion;
  calls.get.mockReset();
});
afterEach(() => { cache.clear(); client.session.setState({ readerVersion: reader }); });

it('erases all cursor pages immediately and cancels an unmounted late result without touching independent scopes', async () => {
  const first = documentDirectoryKey(request), second = documentDirectoryKey({ ...request, cursor: 'next' });
  const retained = documentDirectoryKey({ ...request, deleted: true });
  const elsewhere = documentDirectoryKey({ ...request, at: { kind: 'container', id: 'independently-shared' } });
  cache.setQueryData(first, snapshot('private-first'));
  cache.setQueryData(second, snapshot('private-second'));
  cache.setQueryData(retained, snapshot('retained-scope'));
  cache.setQueryData(elsewhere, snapshot('separately-readable'));
  const late = deferred<DocumentTreePage>(); calls.get.mockReturnValueOnce(late.promise);
  const loading = cache.query({ ...documentDirectoryQuery(request, cache), staleTime: 0 }).catch(() => undefined);
  retireDocumentDirectory(cache, request, reader, new Error('access withdrawn'));
  expect(cache.getQueryData(first)).toBeUndefined();
  expect(cache.getQueryData(second)).toBeUndefined();
  late.resolve(snapshot('late-private')); await loading;
  expect(cache.getQueryData(first)).toBeUndefined();
  expect(cache.getQueryData(retained)).toEqual(snapshot('retained-scope'));
  expect(cache.getQueryData(elsewhere)).toEqual(snapshot('separately-readable'));
});

it('lets a later-page restart supersede another observer’s pending first-page HTTP cut', async () => {
  const late = deferred<DocumentTreePage>(); calls.get.mockReturnValueOnce(late.promise);
  const loading = cache.query(documentDirectoryQuery(request, cache)).catch(() => undefined);
  const current = snapshot('current-visible');
  const adopted = adoptDocumentDirectory(cache, { ...request, cursor: 'old-page' }, current, reader, true);
  expect(adopted?.cursor).toBeNull();
  late.resolve(snapshot('obsolete-visible')); await loading;
  expect(cache.getQueryData(documentDirectoryKey(request))).toEqual(current);
  expect(calls.get).toHaveBeenCalledTimes(1);
});

it('does not apply an old HTTP refusal after a current frame cancelled that read', async () => {
  const late = deferred<DocumentTreePage>(); calls.get.mockReturnValueOnce(late.promise);
  const loading = cache.query(documentDirectoryQuery(request, cache)).catch(() => undefined);
  const current = snapshot('current-grant');
  adoptDocumentDirectory(cache, request, current, reader, false);
  late.reject(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: '', detail: '', request_id: '' }, null));
  await loading; await Promise.resolve();
  expect(cache.getQueryData(documentDirectoryKey(request))).toEqual(current);
});

it('retains the actual HTTP refusal while clearing its previously cached data', async () => {
  cache.setQueryData(documentDirectoryKey(request), snapshot('private'));
  const denied = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: '', detail: '', request_id: '' }, null);
  calls.get.mockRejectedValueOnce(denied);
  await expect(cache.query({ ...documentDirectoryQuery(request, cache), staleTime: 0 })).rejects.toBe(denied);
  expect(cache.getQueryData(documentDirectoryKey(request))).toBeUndefined();
  expect(cache.getQueryState(documentDirectoryKey(request))?.error).toBe(denied);
});

it('adopts a moved folder’s current anchor and retires its old anchor aliases before the next page', async () => {
  cache.setQueryData(documentDirectoryKey(request), snapshot('old-anchor'));
  const moved = snapshot('same-folder-new-node', 'node-b');
  const selected = adoptDocumentDirectory(cache, request, moved, reader, true);
  if (selected === null) throw new Error('The same typed folder is admitted');
  expect(selected.node).toBe('node-b');
  expect(cache.getQueryData(documentDirectoryKey(request))).toBeUndefined();
  expect(cache.getQueryData(documentDirectoryKey(selected))).toEqual(moved);
  calls.get.mockResolvedValueOnce(moved);
  await cache.query(documentDirectoryQuery({ ...selected, cursor: 'next' }, cache));
  const options: unknown = calls.get.mock.calls.at(-1)?.[1];
  expect(calls.get.mock.calls.at(-1)?.[0]).toBe('/nodes/{id}/document-tree');
  expect(options).toMatchObject({ params: { path: { id: 'node-b' } } });
});

it('does not let an old reader erase or replace a new reader’s page and normalizes virtual root identity', () => {
  const current = snapshot('new-reader'); cache.setQueryData(documentDirectoryKey(request), current);
  client.session.setState({ readerVersion: (reader ?? 0) + 1 });
  retireDocumentDirectory(cache, request, reader, new Error('old refusal'));
  expect(adoptDocumentDirectory(cache, request, snapshot('old-reader'), reader, false)).toBeNull();
  expect(cache.getQueryData(documentDirectoryKey(request))).toEqual(current);
  expect(documentDirectoryKey({ node: 'wrong-anchor', at: { kind: 'node_docs', id: 'actual-root' } })[1]).toBe('actual-root');
});
