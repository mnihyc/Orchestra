/** A superseded or cancelled read has no failure to present and must release its loading state. */
import { CancelledError, hashKey, notifyManager, QueryObserver, type Query, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { NetworkError } from '@/lib/requests';
import { beginReadAttempt, failReadAttempt, noteReadError, retireReadAttempt, scheduleReadRetry } from '@/lib/readAttempts';

const retainers = new WeakSet<object>();
const visibility = new WeakMap<object, () => boolean>();
const demand = new WeakMap<object, number>();
const awaited = new WeakMap<Query, { count: number; release: () => void }>();
interface ReadFlight { promise: Query['promise']; before: Query['state'] }
const flights = new WeakMap<Query, ReadFlight>();
const admittedCommits = new WeakSet<object>();
const observedCaches = new WeakSet<object>();
const newerPublication = (before: Query['state'], after: Query['state']) => after.dataUpdateCount !== before.dataUpdateCount
  || after.data !== before.data || after.error !== before.error || after.status !== before.status;

/** Query's own accepted retry event follows delay/eligibility evaluation; no second scheduler is created. */
export function observeReadAttempts(cache: QueryClient): void {
  const owner = cache.getQueryCache();
  if (observedCaches.has(owner)) return;
  observedCaches.add(owner);
  owner.subscribe((event) => {
    if (event.type === 'removed') { retireReadAttempt(event.query); return; }
    if (event.type !== 'updated') return;
    if (event.action.type === 'failed') scheduleReadRetry(event.query, event.action.error);
    else if (event.action.type === 'error') failReadAttempt(event.query, event.action.error);
    else if (event.action.type === 'success' && event.action.manual !== true
      || event.action.type === 'setState' && event.query.state.fetchStatus === 'idle') retireReadAttempt(event.query);
  });
}

/** A native protocol with an admission-checked final merge owns the complete infinite-data commit. */
export function admittedReadCommit(commit: (before: unknown, next: unknown) => unknown): typeof commit {
  admittedCommits.add(commit); return commit;
}

/** Synchronous members from this current supplier do not supersede their own aggregate.
 * An already newer publication remains newer; this cannot acknowledge it on the supplier's behalf. */
export function publishInRead<T>(cache: QueryClient, key: QueryKey, publish: () => T): T {
  const query = cache.getQueryCache().find({ queryKey: key, exact: true }), flight = query === undefined ? undefined : flights.get(query);
  const current = query !== undefined && flight !== undefined && query.promise === flight.promise && !newerPublication(flight.before, query.state);
  const value = publish();
  if (current && query.promise === flight.promise) flight.before = query.state;
  return value;
}

export function retainRead(observer: object): void { retainers.add(observer); }
export function isReadRetainer(observer: object): boolean { return retainers.has(observer); }
export function readVisibility(options: object, shown: () => boolean): void { visibility.set(options, shown); }

/** Models retain data; displayed readers and explicitly awaited work retain transport. */
export function visibleReadCount(query: Pick<Query, 'observers'>): number {
  return query.observers.filter((observer) => !retainers.has(observer) && (visibility.get(observer.options)?.() ?? true)).length;
}

export function hasReadConsumer(query: Pick<Query, 'observers'> & { state: Pick<Query['state'], 'fetchMeta'> }): boolean {
  return (demand.get(query) ?? 0) > 0 || query.state.fetchMeta?.fetchMore !== undefined
    || visibleReadCount(query) > 0;
}

/** A source can pause its demand while preserving the same retained model. */
export function readDemand(query: object, initiallyShown = true): { shown: (shown: boolean) => void; release: () => void } {
  let held = false;
  const shown = (next: boolean) => {
    if (held === next) return;
    held = next;
    const count = (demand.get(query) ?? 0) + (next ? 1 : -1);
    if (count === 0) demand.delete(query); else demand.set(query, count);
  };
  shown(initiallyShown);
  return { shown, release: () => shown(false) };
}

export function holdAwaitedRead(cache: QueryClient, query: Query): () => void {
  let entry = awaited.get(query);
  if (entry === undefined) {
    const wanted = readDemand(query);
    entry = { count: 0, release: wanted.release };
    awaited.set(query, entry);
    const observer = new QueryObserver(cache, { ...query.options, queryKey: query.queryKey, enabled: false, staleTime: Infinity, refetchInterval: false });
    retainRead(observer);
    const stop = observer.subscribe(() => undefined);
    entry.release = () => { wanted.release(); stop(); };
  }
  entry.count += 1;
  const held = entry;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    held.count -= 1;
    if (held.count !== 0) return;
    awaited.delete(query); held.release();
  };
}

export function supersededRead(): CancelledError { return new CancelledError({ revert: true }); }

export function isReadCancelled(error: unknown): boolean {
  return error instanceof CancelledError || error instanceof NetworkError && error.failure === 'aborted'
    || error instanceof DOMException && error.name === 'AbortError'
    || error instanceof Error && error.name === 'AbortError';
}

/** The request that owns a separately shown part, including retained data after a failed refresh. */
export interface RecoverableRead {
  data?: unknown;
  error?: unknown;
  isError?: boolean;
  isFetching?: boolean;
  refetch?: (options?: { cancelRefetch?: boolean }) => unknown;
}

export function hasReadFailure(read: Pick<RecoverableRead, 'isError' | 'error'>): boolean {
  return read.isError === true && !isReadCancelled(read.error);
}

/** A known failed read cannot lend its retained data to an editor or claim current authority. */
export function readData<T>(read: { data: T; isError?: boolean; error?: unknown }): T | undefined {
  return hasReadFailure(read) ? undefined : read.data;
}

/** Returning to a failed part retries its exact owner once; a failure while shown stays for explicit Retry.
 * Refetching through the observer joins an existing flight, and cancellation remains the read owner's concern. */
export function useReadRecovery(read: RecoverableRead, shown = true, identity?: unknown): void {
  const key = hashKey([identity]);
  const previous = useRef({ shown: false, key });
  const failure = hasReadFailure(read);
  const { isFetching, refetch } = read;
  useEffect(() => {
    const returned = shown && (!previous.current.shown || previous.current.key !== key);
    previous.current = { shown, key };
    if (returned && failure && isFetching !== true) void refetch?.({ cancelRefetch: false });
  }, [shown, key, failure, isFetching, refetch]);
}

/** Side data published by a reader belongs to that same flight, even when its transport ignores abort. */
export function queryIsCurrent(cache: QueryClient, key: QueryKey, signal?: AbortSignal): () => boolean {
  const owner = cache.getQueryCache().find({ queryKey: key, exact: true });
  const flight = owner?.promise;
  return () => signal?.aborted !== true && owner?.promise === flight
    && cache.getQueryCache().find({ queryKey: key, exact: true }) === owner;
}

/** Stop transport while retaining the latest publication, including one supplied during that read. */
export function cancelRead(query: Query): Promise<void> {
  if (query.promise === undefined) return Promise.resolve();
  const held = query.state;
  return notifyManager.batch(() => {
    const cancelled = query.cancel({ revert: true });
    query.setState({ ...held, fetchStatus: 'idle' });
    return cancelled;
  });
}

/** Query cancellation must go through its owner: merely throwing a silent error leaves its fetch busy. */
export async function queryRead<T>(cache: QueryClient, hash: string, read: () => T | Promise<T>, reader = ''): Promise<T> {
  const owner = cache.getQueryCache().get(hash);
  const flight = owner?.promise;
  if (owner !== undefined) beginReadAttempt(owner, flight, reader);
  const cut = owner === undefined ? undefined : { promise: flight, before: owner.state };
  if (owner !== undefined && cut !== undefined) flights.set(owner, cut);
  const published = () => owner !== undefined && cut !== undefined && newerPublication(cut.before, owner.state);
  const guardedCommit = () => typeof owner?.options.structuralSharing === 'function' && admittedCommits.has(owner.options.structuralSharing);
  const nativeFailure = () => owner?.state.data === undefined && owner?.state.status === 'error' ? owner.state.error : null;
  const retire = async () => {
    if (owner !== undefined && owner.promise === flight && cache.getQueryCache().get(hash) === owner) await cancelRead(owner);
  };
  try {
    const answer = await read();
    // A query function may return one infinite page; returning the cache's whole value here would
    // corrupt that page shape. Retire the old flight so its query keeps the newer publication.
    if (published() && !guardedCommit()) {
      const error = nativeFailure();
      if (error !== null) throw error;
      await retire(); throw supersededRead();
    }
    return answer;
  }
  catch (error) {
    if (owner !== undefined && !isReadCancelled(error)) noteReadError(owner, flight, error);
    if (!isReadCancelled(error) && !published()) throw error;
    const failure = nativeFailure();
    if (published() && failure !== null) throw failure;
    await retire();
    throw supersededRead();
  } finally { if (owner !== undefined && flights.get(owner) === cut) flights.delete(owner); }
}
