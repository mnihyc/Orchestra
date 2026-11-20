/** Admission/aggregate ownership controls, independent of HTTP timing and screen counts. */
import { expect, it, vi } from 'vitest';
import { ReadScheduler } from '@/api/readScheduler';
import { LiveQueryClient } from '@/api/query';
import { publishInRead, supersededRead } from '@/api/reads';
import { QueryObserver } from '@tanstack/react-query';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

it('finds and retires a supplied read without inspecting another resource family or owner', () => {
  const scheduler = new ReadScheduler();
  const word = vi.fn(() => 'key');
  const kept = ['first-owner', { toJSON: word }];
  scheduler.supply(kept, [['people', 1]]);
  scheduler.supply(['next-owner'], [['places', 2]]);
  word.mockClear();
  expect(scheduler.owns(['another-family', { toJSON: word }])).toBe(false);
  scheduler.release(['next-owner']);
  expect(word).not.toHaveBeenCalled();
  expect(scheduler.owns(['people', 1])).toBe(true);
  scheduler.supply(['replacement'], [['people', 1]]);
  scheduler.release(kept);
  expect(scheduler.owner(['people', 1])).toEqual(['replacement']);
});

it('uses the native query hash to start and retain an exact read beside five thousand cached answers', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  for (let i = 0; i < 5_000; i++) cache.setQueryData(['held', i], i);
  const scan = vi.spyOn(cache.getQueryCache(), 'getAll');
  try {
    const options = { queryKey: ['open'] as const, queryKeyHashFn: () => 'custom-native-owner', queryFn: () => Promise.resolve('fresh') };
    await expect(cache.query(options)).resolves.toBe('fresh');
    expect(cache.getQueryCache().get('custom-native-owner')?.state.data).toBe('fresh');
    expect(scan).not.toHaveBeenCalled();
  } finally { scan.mockRestore(); cache.clear(); }
});

it('binds a reused query function to the native answer of each overlapping read', async () => {
  const cache = new LiveQueryClient();
  const one = deferred<string>(), two = deferred<string>();
  const raw = vi.fn<() => Promise<string>>().mockReturnValueOnce(one.promise).mockReturnValueOnce(two.promise);
  const key = ['shared-reader'];
  const first = cache.defaultQueryOptions({ queryKey: key, queryHash: 'first-answer', queryFn: raw });
  try {
    const firstRead = cache.query({ queryKey: key, queryHash: 'first-answer', queryFn: first.queryFn });
    const secondRead = cache.query({ queryKey: key, queryHash: 'second-answer', queryFn: first.queryFn });
    const firstQuery = cache.getQueryCache().get<string>('first-answer');
    expect(firstQuery).toBeDefined();
    firstQuery?.setData('Newer first answer');
    one.resolve('Late first answer'); two.resolve('Second answer');
    expect(await firstRead).toBe('Newer first answer');
    expect(await secondRead).toBe('Second answer');
  } finally { one.resolve(''); two.resolve(''); cache.clear(); }
});

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

it('preserves the complete infinite-query shape when a live publication overtakes a successful page read', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['retained-infinite-publication'], pending = deferred<string>();
  cache.setQueryData(key, { pages: ['Earlier page'], pageParams: [0] });
  const reading = cache.infiniteQuery({ queryKey: key, initialPageParam: 0, getNextPageParam: () => undefined, staleTime: 0, queryFn: () => pending.promise });
  try {
    cache.setQueryData(key, { pages: ['Newer first page', 'Retained second page'], pageParams: [0, 1] });
    pending.resolve('Obsolete page');
    expect(await reading).toEqual({ pages: ['Newer first page', 'Retained second page'], pageParams: [0, 1] });
    expect(cache.getQueryState(key)).toMatchObject({ status: 'success', error: null, fetchStatus: 'idle' });
  } finally { pending.resolve('Obsolete page'); cache.clear(); }
});

it.each([false, true])('finishes its own aggregate publication while preserving a later native answer=%s', async (external) => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['aggregate-publication'], pending = deferred<string>();
  cache.setQueryData(key, 'Earlier answer');
  const reading = cache.query({ queryKey: key, staleTime: 0, queryFn: async () => {
    publishInRead(cache, key, () => cache.setQueryData(key, 'Admitted aggregate member'));
    return await pending.promise;
  } });
  try {
    if (external) cache.setQueryData(key, 'Newer native answer');
    pending.resolve('Complete aggregate');
    expect(await reading).toBe(external ? 'Newer native answer' : 'Complete aggregate');
    expect(cache.getQueryState(key)).toMatchObject({ status: 'success', fetchStatus: 'idle', error: null });
  } finally { pending.resolve('Complete aggregate'); cache.clear(); }
});

it('keeps an authoritative removal when its former successful HTTP read finishes late', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const key = ['removed-native-row'], pending = deferred<string>();
  cache.setQueryData(key, 'Previously permitted row');
  const reading = cache.query({ queryKey: key, staleTime: 0, queryFn: () => pending.promise }).catch(() => undefined);
  const query = cache.getQueryCache().find({ queryKey: key, exact: true });
  try {
    const refusal = new Error('No longer permitted');
    query?.setState({ data: undefined, error: refusal, status: 'error', fetchStatus: 'idle' });
    pending.resolve('Former private row'); await reading;
    expect(cache.getQueryState(key)).toMatchObject({ data: undefined, error: refusal, status: 'error', fetchStatus: 'idle' });
  } finally { pending.resolve('Former private row'); cache.clear(); }
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

it('lets an authority refresh follow a retained cache lookup without making the answer static', async () => {
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const pending = deferred<string>(), key = ['retained-authority-read'];
  const read = vi.fn<() => Promise<string>>().mockImplementationOnce(() => pending.promise).mockResolvedValue('Current authority');
  const reading = cache.query({ queryKey: key, queryFn: read, staleTime: 'static' });
  const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
  const stop = observer.subscribe(() => undefined);
  const initial = observer.getCurrentQuery().promise;
  const refreshed = initial?.then(() => cache.invalidateQueries({ queryKey: key, exact: true }));
  try {
    pending.resolve('Previous authority');
    await Promise.all([reading, refreshed]);
    expect(read).toHaveBeenCalledTimes(2);
    expect(cache.getQueryData(key)).toBe('Current authority');
  } finally { stop(); pending.resolve('Previous authority'); cache.clear(); }
});
