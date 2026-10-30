/** Current access and related-ranking changes retire the cached list without turning unrelated settings into reads. */
import { InfiniteQueryObserver, QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, findAct } from '@/api/acts';

it.each([
  ['PUT', '/nodes/place/settings/related.weights'],
  ['PUT', '/nodes/place/settings/search.semantic.enabled'],
  ['PATCH', '/documents/candidate/permissions'],
  ['POST', '/admin/semantic-search/root/rebuild'],
])('retakes a visible related list after %s %s', async (method, path) => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['document-related', 'source', 1];
  cache.setQueryData(key, { items: ['candidate'] });
  const read = vi.fn(() => Promise.resolve({ items: [] }));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct(method, path)!.changes, cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toEqual({ items: [] });
  } finally { stop(); cache.clear(); }
});

it('keeps a related revision cached when an unrelated setting changes', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['document-related', 'source', 1];
  cache.setQueryData(key, { items: ['candidate'] });
  const read = vi.fn(() => Promise.resolve({ items: [] }));
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(findAct('PUT', '/nodes/place/settings/thread.side_threads')!.changes, cache);
    expect(read).not.toHaveBeenCalled();
    expect(cache.getQueryData(key)).toEqual({ items: ['candidate'] });
  } finally { stop(); cache.clear(); }
});

it.each([true, false])('refreshes only a displayed list after membership news, displayed=%s', async (shown) => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['document-related', 'source', 1];
  cache.setQueryData(key, { items: ['candidate'] });
  const read = vi.fn(() => Promise.resolve({ items: [] }));
  const stop = shown ? new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined) : () => undefined;
  try {
    await applyChanges([{ people: true }], cache);
    expect(read).toHaveBeenCalledTimes(shown ? 1 : 0);
    expect(cache.getQueryData(key)).toEqual(shown ? { items: [] } : undefined);
  } finally { stop(); cache.clear(); }
});

it('cancels an older related answer before applying a new access cut', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['document-related', 'source', 1];
  cache.setQueryData(key, { items: ['candidate'] });
  let finish: ((value: { items: string[] }) => void) | undefined;
  let signal: AbortSignal | undefined;
  const old = new Promise<{ items: string[] }>((resolve) => { finish = resolve; });
  const read = vi.fn().mockImplementationOnce(({ signal: given }: { signal: AbortSignal }) => { signal = given; return old; }).mockResolvedValue({ items: [] });
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  const refresh = cache.refetchQueries({ queryKey: key, exact: true });
  try {
    await applyChanges(findAct('PATCH', '/documents/candidate/permissions')!.changes, cache);
    expect(signal?.aborted).toBe(true);
    expect(cache.getQueryData(key)).toEqual({ items: [] });
    finish?.({ items: ['candidate'] });
    await old; await refresh;
    expect(cache.getQueryData(key)).toEqual({ items: [] });
  } finally { stop(); cache.clear(); }
});

it('retires every old continuation before a fresh access read', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });
  const key = ['document-related', 'paged-source', 1];
  cache.setQueryData(key, { pages: [{ items: ['old-first'], next_cursor: 'old-next' }, { items: ['old-private'], next_cursor: null }], pageParams: [null, 'old-next'] });
  const read = vi.fn(() => Promise.resolve({ items: ['current'], next_cursor: null as string | null }));
  const stop = new InfiniteQueryObserver(cache, { queryKey: key, initialPageParam: null as string | null,
    queryFn: read, getNextPageParam: (page) => page.next_cursor ?? undefined }).subscribe(() => undefined);
  try {
    await applyChanges([{ relatedViews: true }], cache);
    expect(read).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(key)).toEqual({ pages: [{ items: ['current'], next_cursor: null }], pageParams: [null] });
  } finally { stop(); cache.clear(); }
});
