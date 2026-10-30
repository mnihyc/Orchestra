/** Actor filtering is part of the native request and cache identity, before every history page. */
import { InfiniteQueryObserver, QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientApi from '@/api/client';
import { historyQuery, PEOPLE_PAGE } from '@/api/access';
import { covers } from '@/api/acts';
import type { HistoryLinePage } from '@/generated/api-types';

interface ReadOptions { params: { path: { object: string; id: string }; query: { limit: number; depth: string; actor?: string; cursor?: string } }; signal: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options: ReadOptions) => Promise<{ data: HistoryLinePage; response: Response }>>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof ClientApi>();
  return { ...actual, api: { ...actual.api, GET: calls.read } };
});
const target = { kind: 'thread' as const, id: '01950000-0000-7000-8000-000000000001' };
const actor = '01950000-0000-7000-8000-000000000002';
const other = '01950000-0000-7000-8000-000000000003';
let cache: QueryClient;
const answer = (text: string, next: string | null = null) => ({ data: { items: [{ id: text, text, action: 'thread.created', actor: null,
  at: '2026-10-18T00:00:00Z', about: null }], next_cursor: next }, response: new Response(null, { status: 200 }) });
beforeEach(() => { calls.read.mockReset(); cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } }); });
afterEach(() => { cache.clear(); });

it('sends the exact actor and unchanged depth/limit on the initial page and native cursor continuation', async () => {
  calls.read.mockResolvedValueOnce(answer('actor first page', 'native-next')).mockResolvedValueOnce(answer('actor second page'));
  const options = historyQuery(target, 'summary', actor), observer = new InfiniteQueryObserver(cache, { ...options, enabled: false });
  await observer.refetch(); await observer.fetchNextPage();
  expect(calls.read.mock.calls.map(([path, value]) => [path, value.params])).toEqual([
    ['/{object}/{id}/history', { path: { object: 'nodes', id: target.id }, query: { limit: PEOPLE_PAGE, depth: 'summary', actor } }],
    ['/{object}/{id}/history', { path: { object: 'nodes', id: target.id }, query: { limit: PEOPLE_PAGE, depth: 'summary', actor, cursor: 'native-next' } }],
  ]);
  expect(calls.read.mock.calls.every(([, value]) => value.signal instanceof AbortSignal)).toBe(true);
  expect(observer.getCurrentResult().data?.pages.flatMap((page) => page.items.map((line) => line.text))).toEqual(['actor first page', 'actor second page']);
  expect(observer.getCurrentResult().data?.pageParams).toEqual([null, 'native-next']);
  observer.destroy();
});

it('isolates actor, unfiltered, depth and target histories while leaving the optional existing caller unfiltered', async () => {
  calls.read.mockImplementation((_path, { params }) => Promise.resolve(answer(JSON.stringify(params))));
  const queries = [historyQuery(target, 'summary', actor), historyQuery(target, 'summary', other), historyQuery(target, 'summary'),
    historyQuery(target, 'every', actor), historyQuery({ ...target, id: other }, 'summary', actor)];
  for (const query of queries) await cache.infiniteQuery(query);
  expect(new Set(queries.map((query) => JSON.stringify(query.queryKey))).size).toBe(queries.length);
  expect(queries.map((query) => cache.getQueryData(query.queryKey))).toHaveLength(5);
  expect(new Set(queries.map((query) => JSON.stringify(cache.getQueryData(query.queryKey)))).size).toBe(5);
  const unfiltered = historyQuery({ kind: 'provider', id: other });
  await cache.infiniteQuery(unfiltered);
  expect(calls.read.mock.calls.at(-1)?.[1].params).toEqual({ path: { object: 'providers', id: other }, query: { limit: PEOPLE_PAGE, depth: 'every' } });
  expect(unfiltered.queryKey).toEqual(['page-history', 'providers', other, 'every', null]);
});

it('keeps existing page-history prefix and target invalidation applicable to all actor variants', () => {
  for (const selected of [undefined, actor, other]) {
    const key = historyQuery(target, 'summary', selected).queryKey;
    expect(covers({ key: ['page-history'] }, key)).toBe(true);
    expect(covers({ key: ['page-history'], id: target.id }, key)).toBe(true);
    expect(covers({ key: ['page-history'], id: 'unrelated-target' }, key)).toBe(false);
  }
});
