// @vitest-environment node
/** After a reconnect, which has said nothing about reach, the views of a person's reach are read again, and a view keeps what it
 * shows meanwhile: the screen showed that data until now, and the re-read is the first moment anything is learnt. What the re-read
 * refuses goes at once; what it returns instead replaces it; what fails for the network or the server stays. An act's answer that
 * says reach changed does say something: it empties the record of the person's access to an org (which the org screens hide, with
 * their state, until it is read again) and holds every other view. */
import { setImmediate as nextTurn } from 'node:timers/promises';
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it } from 'vitest';
import { ApiError } from '@/api/client';
import { repairPeople } from '@/api/peopleChanges';
import { NetworkError } from '@/lib/requests';

const key = ['node-chat-opening', 'place'] as const;
const other = ['node-chat-opening', 'other'] as const;

function gate<T>() {
  let resolve!: (value: T) => void, reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const refused = (status: number, code: 'authz.denied' | 'not_found') => new ApiError({ type: `orch:error:${code}`, status, code, title: 'Refused', detail: 'The read is refused.', request_id: `refused-${status}` }, null);

const open: (() => void)[] = [];
afterEach(() => { for (const stop of open.splice(0)) stop(); });

function watch(cache: QueryClient, name: readonly string[], read: () => Promise<unknown>) {
  const observer = new QueryObserver(cache, { queryKey: name, queryFn: read, staleTime: Infinity, retry: false });
  open.push(observer.subscribe(() => undefined));
  return observer;
}
function held(name: readonly string[] = key) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const first = { members: [{ id: 'ada' }, { id: 'dora' }], count: 2 };
  cache.setQueryData(name, first);
  return { cache, first };
}

it('keeps what a view shows while it is read again, and keeps the very same data when the re-read says the same', async () => {
  const { cache, first } = held();
  const answer = gate<unknown>();
  const observer = watch(cache, key, () => answer.promise);
  const repairing = repairPeople(cache, [], undefined);
  await nextTurn();
  expect(observer.getCurrentResult()).toMatchObject({ status: 'success', fetchStatus: 'fetching' });
  expect(observer.getCurrentResult().data).toBe(first);
  answer.resolve({ members: [{ id: 'ada' }, { id: 'dora' }], count: 2 });
  await repairing;
  // Structural sharing: nothing that shows it is drawn again.
  expect(cache.getQueryData(key)).toBe(first);
});

it('replaces what it holds with what the re-read returns instead', async () => {
  const { cache } = held();
  const observer = watch(cache, key, () => Promise.resolve({ members: [{ id: 'ada' }], count: 1 }));
  await repairPeople(cache, [], undefined);
  expect(observer.getCurrentResult().data).toEqual({ members: [{ id: 'ada' }], count: 1 });
});

for (const [status, code] of [[403, 'authz.denied'], [404, 'not_found']] as const) {
  it(`gives it up as soon as the re-read is refused (${status}), without waiting for the others`, async () => {
    const { cache, first } = held();
    const slow = gate<unknown>();
    cache.setQueryData(other, { members: [] });
    const refusing = watch(cache, key, () => Promise.reject(refused(status, code)));
    watch(cache, other, () => slow.promise);
    const repairing = repairPeople(cache, [], undefined);
    await expect.poll(() => refusing.getCurrentResult().data, { interval: 5 }).toBeUndefined();
    expect(refusing.getCurrentResult()).toMatchObject({ status: 'error', isError: true });
    expect(first.count).toBe(2);
    // The other view is still being read, and still shows what it had.
    expect(cache.getQueryData(other)).toEqual({ members: [] });
    slow.resolve({ members: [] });
    await repairing;
  });
}

it('keeps what it shows when the re-read fails for the network or the server, and says so', async () => {
  for (const failure of [new NetworkError('offline'), new ApiError({ type: 'orch:error:internal', status: 500, code: 'internal', title: 'Failed', detail: 'Failed', request_id: 'server' }, null)]) {
    const { cache, first } = held();
    const observer = watch(cache, key, () => Promise.reject(failure));
    await repairPeople(cache, [], undefined);
    expect(observer.getCurrentResult().data).toBe(first);
    expect(observer.getCurrentResult().isError).toBe(true);
    for (const stop of open.splice(0)) stop();
  }
});

it('pauses only the record of a person\'s access to an org when an act\'s answer says that reach changed, and holds every other view', async () => {
  const { cache, first } = held();
  const org = ['org', 'one'] as const;
  cache.setQueryData(org, { access: { permissions: ['members.manage'] } });
  const answer = gate<unknown>(), access = gate<unknown>();
  const view = watch(cache, key, () => answer.promise);
  const record = watch(cache, org, () => access.promise);
  const repairing = repairPeople(cache, [], undefined, true);
  await nextTurn();
  expect(record.getCurrentResult()).toMatchObject({ status: 'pending', fetchStatus: 'fetching', data: undefined });
  expect(view.getCurrentResult()).toMatchObject({ status: 'success', fetchStatus: 'fetching' });
  expect(view.getCurrentResult().data).toBe(first);
  answer.resolve({ members: [], count: 0 }); access.resolve({ access: { permissions: [] } });
  await repairing;
  expect(view.getCurrentResult().data).toEqual({ members: [], count: 0 });
  expect(record.getCurrentResult().data).toEqual({ access: { permissions: [] } });
});

it('pauses nothing after a reconnect, not even the record of a person\'s access to an org', async () => {
  const { cache } = held(['org', 'one']);
  const access = gate<unknown>();
  const record = watch(cache, ['org', 'one'], () => access.promise);
  const repairing = repairPeople(cache, [], undefined);
  await nextTurn();
  expect(record.getCurrentResult()).toMatchObject({ status: 'success', fetchStatus: 'fetching' });
  access.resolve({ members: [{ id: 'ada' }, { id: 'dora' }], count: 2 });
  await repairing;
});

it('removes a view that nobody is looking at without reading it', async () => {
  const { cache } = held();
  let reads = 0;
  cache.setQueryDefaults(key, { queryFn: () => { reads += 1; return Promise.resolve(null); } });
  await repairPeople(cache, [], undefined);
  expect(reads).toBe(0);
  expect(cache.getQueryCache().find({ queryKey: key })).toBeUndefined();
});
