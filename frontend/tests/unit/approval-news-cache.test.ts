/** Approval news carries counters, so shown approval projections must retake their own current rows. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import { applyChanges, userEventChanges } from '@/api/acts';
import { client } from '@/api/client';

const news = () => userEventChanges('approvals', { summary: { unread: 0, approvals_pending: 0 } });
const cacheOf = () => new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false } } });

it('retakes each shown list and exact request once, expires hidden approvals and leaves other views unread', async () => {
  const cache = cacheOf();
  const lists = [['approvals', 'mine', 'pending', false, null], ['approvals', 'requested', 'decided', false, null], ['approval', 'shown']] as const;
  const reads = lists.map((key) => {
    cache.setQueryData(key, { state: 'pending' });
    return vi.fn(() => Promise.resolve({ state: 'denied', decided_by: 'Grace' }));
  });
  const stops = lists.map((key, at) => new QueryObserver(cache, { queryKey: key, queryFn: reads[at] }).subscribe(() => undefined));
  const hidden = ['approvals', 'mine', 'decided', false, null];
  const exactHidden = ['approval', 'hidden'];
  cache.setQueryData(hidden, { state: 'pending' });
  cache.setQueryData(exactHidden, { state: 'pending' });
  const other = vi.fn(() => Promise.resolve('unchanged'));
  cache.setQueryData(['thread', 'shown'], 'unchanged');
  stops.push(new QueryObserver(cache, { queryKey: ['thread', 'shown'], queryFn: other }).subscribe(() => undefined));
  try {
    await Promise.all([applyChanges(news(), cache), applyChanges(news(), cache)]);
    for (const [at, key] of lists.entries()) {
      expect(cache.getQueryData(key)).toEqual({ state: 'denied', decided_by: 'Grace' });
      expect(reads[at]).toHaveBeenCalledTimes(1);
    }
    expect(cache.getQueryData(hidden)).toBeUndefined();
    expect(cache.getQueryData(exactHidden)).toBeUndefined();
    expect(cache.getQueryData(['thread', 'shown'])).toBe('unchanged');
    expect(other).not.toHaveBeenCalled();
  } finally { stops.forEach((stop) => stop()); cache.clear(); }
});

it('cancels an older pending list read before the current decision can replace it', async () => {
  const cache = cacheOf();
  const key = ['approvals', 'mine', 'pending', false, null];
  let finish!: (value: string[]) => void;
  const old = new Promise<string[]>((resolve) => { finish = resolve; });
  let oldSignal: AbortSignal | undefined;
  const read = vi.fn(({ signal }: { signal: AbortSignal }) => {
    if (oldSignal === undefined) { oldSignal = signal; return old; }
    return Promise.resolve([]);
  });
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  try {
    await applyChanges(news(), cache);
    expect(oldSignal?.aborted).toBe(true);
    expect(cache.getQueryData(key)).toEqual([]);
    finish(['a request that already ended']);
    await old;
    expect(cache.getQueryData(key)).toEqual([]);
    expect(read).toHaveBeenCalledTimes(2);
  } finally { stop(); cache.clear(); }
});

it('does not read a replaced reader after retiring its old query', async () => {
  const cache = cacheOf();
  const key = ['approval', 'shown'];
  const read = vi.fn(() => Promise.resolve('current'));
  cache.setQueryData(key, 'old');
  const stop = new QueryObserver(cache, { queryKey: key, queryFn: read }).subscribe(() => undefined);
  const before = client.session.getState().readerVersion;
  const cancel = cache.cancelQueries.bind(cache);
  vi.spyOn(cache, 'cancelQueries').mockImplementation(async (...args) => {
    await cancel(...args);
    client.session.setState({ readerVersion: (before ?? 0) + 1 });
  });
  try {
    await applyChanges(news(), cache);
    expect(read).not.toHaveBeenCalled();
  } finally { client.session.setState({ readerVersion: before }); stop(); cache.clear(); }
});
