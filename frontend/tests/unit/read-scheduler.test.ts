/** Admission/aggregate ownership controls, independent of HTTP timing and screen counts. */
import { expect, it, vi } from 'vitest';
import { ReadScheduler } from '@/api/readScheduler';
import { LiveQueryClient } from '@/api/query';
import { supersededRead } from '@/api/reads';
import { QueryObserver } from '@tanstack/react-query';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

it('shares overlapping suppliers and releases a failed supplier for an explicit retry', async () => {
  const scheduler = new ReadScheduler(); const pending = deferred<string>();
  const read = vi.fn(() => pending.promise);
  const one = scheduler.run(['overview', 'org'], read);
  const two = scheduler.run(['overview', 'org'], read);
  expect(one).toBe(two);
  await Promise.resolve(); expect(read).toHaveBeenCalledOnce();
  pending.resolve('native page'); expect(await two).toBe('native page');
  await expect(scheduler.run(['failed'], () => Promise.reject(new Error('offline')))).rejects.toThrow('offline');
  await expect(scheduler.run(['failed'], () => Promise.resolve('repaired'))).resolves.toBe('repaired');
});

it('retires an old reader even when its transport ignores cancellation and finishes last', async () => {
  const scheduler = new ReadScheduler(); const old = deferred<string>(); const published: string[] = [];
  const former = scheduler.run(['overview'], async (current) => { const data = await old.promise; if (current()) published.push(data); return data; });
  await Promise.resolve(); scheduler.advance();
  await scheduler.run(['overview'], (current) => { if (current()) published.push('current'); return Promise.resolve('current'); });
  old.resolve('former'); await former;
  expect(published).toEqual(['current']);
});

it('one aggregate supplies exact native pages until an authority cut and keeps their repair owner', () => {
  const scheduler = new ReadScheduler(); const owner = ['org-overview', 'org', { participating: false }];
  const page = ['children', 'org', 'exclude', false]; const visits = ['names', 'thread', '', 50, 'org', false, false];
  scheduler.supply(owner, [page, visits]);
  expect(scheduler.owns(page)).toBe(true); expect(scheduler.owns(visits)).toBe(true);
  expect(scheduler.owns(['children', 'org', 'exclude', true])).toBe(false);
  expect(scheduler.owns(['children', 'other', 'exclude', false])).toBe(false);
  scheduler.advance(); expect(scheduler.owns(page)).toBe(false); expect(scheduler.owner(page)).toEqual(owner);
  scheduler.supply(owner, [page]); expect(scheduler.owns(page)).toBe(true);
  scheduler.release(owner); expect(scheduler.owner(page)).toBeUndefined();
});

it('drops a superseded read without an error or a stuck loading state and reads the same key on return', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['superseded-section', 'kept-view'];
  try {
    await expect(cache.query({ queryKey: key, queryFn: () => Promise.reject(supersededRead()) })).rejects.toBeInstanceOf(Error);
    expect(cache.getQueryState(key)).toMatchObject({ status: 'pending', fetchStatus: 'idle', error: null });
    expect(await cache.query({ queryKey: key, queryFn: () => Promise.resolve('Newest answer') })).toBe('Newest answer');
  } finally { cache.clear(); }
});

it('lets a newer read finish when an older transport ignores cancellation and rejects late', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['superseded-section', 'same-key'];
  const older = deferred<string>(), newer = deferred<string>();
  cache.setQueryData(key, 'Kept answer');
  try {
    const former = cache.query({ queryKey: key, staleTime: 0, queryFn: async () => { await older.promise; throw supersededRead(); } }).catch(() => undefined);
    await cache.cancelQueries({ queryKey: key, exact: true });
    const latest = cache.query({ queryKey: key, staleTime: 0, queryFn: () => newer.promise });
    older.resolve('Former answer');
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(cache.getQueryState(key)?.fetchStatus).toBe('fetching');
    newer.resolve('Newest answer');
    expect(await latest).toBe('Newest answer');
    await former;
  } finally { newer.resolve('Newest answer'); older.resolve('Former answer'); cache.clear(); }
});

it('preserves a newer supplied answer when an older read is superseded before it settles', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['superseded-section', 'supplied-answer'];
  const pending = deferred<string>();
  cache.setQueryData(key, 'Earlier answer');
  try {
    const older = cache.query({ queryKey: key, staleTime: 0, queryFn: async () => { await pending.promise; throw supersededRead(); } });
    cache.setQueryData(key, 'Newer supplied answer');
    pending.resolve('Obsolete answer');
    expect(await older).toBe('Newer supplied answer');
    expect(cache.getQueryState(key)).toMatchObject({ data: 'Newer supplied answer', status: 'success', fetchStatus: 'idle', error: null });
  } finally { pending.resolve('Obsolete answer'); cache.clear(); }
});

it('keeps an imperative reader alive when the last displayed observer leaves', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const pending = deferred<string>();
  let signal: AbortSignal | undefined;
  const options = { queryKey: ['shared-imperative-read'], queryFn: ({ signal: current }: { signal: AbortSignal }) => { signal = current; return pending.promise; } };
  const observer = new QueryObserver(cache, options);
  const stop = observer.subscribe(() => undefined);
  const reading = cache.query(options).catch((error: unknown) => error);
  try {
    stop();
    expect(signal?.aborted).toBe(false);
    pending.resolve('The requested answer');
    expect(await reading).toBe('The requested answer');
  } finally { stop(); pending.resolve('The requested answer'); cache.clear(); }
});
