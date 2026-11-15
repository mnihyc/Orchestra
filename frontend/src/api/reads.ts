/** A superseded or cancelled read has no failure to present and must release its loading state. */
import { CancelledError, notifyManager, QueryObserver, type Query, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { NetworkError } from '@/lib/requests';

const retainers = new WeakSet<object>();
const visibility = new WeakMap<object, () => boolean>();
const demand = new WeakMap<object, number>();
const awaited = new WeakMap<Query, { count: number; release: () => void }>();
interface ReadFlight { promise: Query['promise']; before: Query['state'] }
const flights = new WeakMap<Query, ReadFlight>();
const admittedCommits = new WeakSet<object>();
const newerPublication = (before: Query['state'], after: Query['state']) => after.dataUpdateCount !== before.dataUpdateCount
  || after.data !== before.data || after.error !== before.error || after.status !== before.status;

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
export async function queryRead<T>(cache: QueryClient, key: QueryKey, read: () => T | Promise<T>): Promise<T> {
  const owner = cache.getQueryCache().find({ queryKey: key, exact: true });
  const flight = owner?.promise;
  const cut = owner === undefined ? undefined : { promise: flight, before: owner.state };
  if (owner !== undefined && cut !== undefined) flights.set(owner, cut);
  const published = () => owner !== undefined && cut !== undefined && newerPublication(cut.before, owner.state);
  const guardedCommit = () => typeof owner?.options.structuralSharing === 'function' && admittedCommits.has(owner.options.structuralSharing);
  const nativeFailure = () => owner?.state.data === undefined && owner?.state.status === 'error' ? owner.state.error : null;
  const retire = async () => {
    if (owner !== undefined && owner.promise === flight && cache.getQueryCache().find({ queryKey: key, exact: true }) === owner) await cancelRead(owner);
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
    if (!isReadCancelled(error) && !published()) throw error;
    const failure = nativeFailure();
    if (published() && failure !== null) throw failure;
    await retire();
    throw supersededRead();
  } finally { if (owner !== undefined && flights.get(owner) === cut) flights.delete(owner); }
}
