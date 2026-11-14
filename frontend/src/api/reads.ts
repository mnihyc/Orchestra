/** A superseded or cancelled read has no failure to present and must release its loading state. */
import { CancelledError, hashKey, notifyManager, QueryObserver, type Query, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { NetworkError } from '@/lib/requests';

const retainers = new WeakSet<object>();
const visibility = new WeakMap<object, () => boolean>();
const demand = new WeakMap<object, number>();
const awaited = new WeakMap<Query, { count: number; release: () => void }>();

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
    const observer = new QueryObserver(cache, { ...query.options, queryKey: query.queryKey, enabled: false, refetchInterval: false });
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
    if (held.fetchStatus !== 'idle') query.setState({ ...held, fetchStatus: 'idle' });
    return cancelled;
  });
}

/** Query cancellation must go through its owner: merely throwing a silent error leaves its fetch busy. */
export async function queryRead<T>(cache: QueryClient, key: QueryKey, read: () => T | Promise<T>): Promise<T> {
  const owner = cache.getQueryCache().find({ queryKey: key, exact: true });
  const flight = owner?.promise;
  try { return await read(); }
  catch (error) {
    if (!isReadCancelled(error)) throw error;
    if (owner !== undefined && owner.promise === flight) {
      await cancelRead(owner);
    }
    throw supersededRead();
  }
}
