/** What was asked of the API and is kept: the one query client, when it asks again, and how an error is told apart.
 *
 * Everything a screen reads over HTTP goes through this client, under a key of the form
 * `[resource, id, params]`: the kind of thing, which one, and how it was asked for. Each module of
 * `src/api` names its own keys beside its requests.
 *
 * A read that failed for the network is asked again three times, after one, two and four seconds;
 * an answer of the API is an answer, and is never asked for again by itself. A write is not repeated
 * here at all: whoever makes one that is safe to repeat says so by its key.
 */
import { QueryClient, type InvalidateOptions, type InvalidateQueryFilters, type QueryClientConfig, type QueryKey } from '@tanstack/react-query';

import { ApiError, client } from '@/api/client';
import { shellBootstrapIndex } from '@/api/bootstrapIndex';
import { resourceRowIndex } from '@/api/resourceRowIndex';
import { scheduledReads, startingRead } from '@/api/readScheduler';
import { NetworkError, retryPause, retryRead } from '@/lib/requests';
import { setOrchLinkSource, type OrchLinkAddresses } from '@/lib/orchLinks';
import { readingPreferences } from '@/lib/interfaceReading';
import { readFreshness, readFreshnessSettings } from '@/api/freshness';
import { holdAwaitedRead, observeReadAttempts, queryRead } from '@/api/reads';
import { observedRetryDelay, requestReader } from '@/lib/readAttempts';

/** How long an answer nobody shows is kept: long enough to come back to a screen and find it there. */
const KEPT_FOR_MS = 5 * 60_000;

/** The marker that every thread read's key carries (`realtime/threadScope.ts`), before its read lifetime and epoch. */
const THREAD_READ = 'continuation-read';

/** What a thread read's key names apart from its read lifetime, epoch and projection: the thing, its window and the
 *  selected version (a first, unopened read follows the active one). `null` for a key that is not a thread read's. */
export function threadReadIdentity(key: QueryKey): string | null {
  const at = key.indexOf(THREAD_READ);
  if (at < 0) return null;
  const selection = key[at + 1] === 'unopened-active' ? null : key[at + 3] ?? null;
  return JSON.stringify([key.slice(0, at), selection]);
}

/**
 * A thread is read again under a new epoch after a reload, a reconnect or a changed history, which gives each of its
 * reads (outline, summary, messages, turns…) a new key. Until the new key answers, a view keeps showing what the same
 * read of the same thing showed, as placeholder data, and then swaps in the new answer. An answer that was retired from
 * the cache (another person, a reset of authority) is never carried over.
 */
function keptThreadRead(cache: QueryClient, key: QueryKey) {
  const identity = threadReadIdentity(key);
  return (previous: unknown, previousQuery?: { queryHash: string; queryKey: QueryKey }): unknown =>
    previousQuery !== undefined && identity !== null && cache.getQueryCache().get(previousQuery.queryHash) === previousQuery
      && threadReadIdentity(previousQuery.queryKey) === identity ? previous : undefined;
}

/** A passing inspection may become kept while its one detail read finishes into the same cache. */
export interface ReadOptions {
  keepReading?: boolean;
}

/** Legacy screen callbacks cannot turn one act into a refresh of every list of its kind.
 * Native act/feed answers patch the cache; a missing projection must name one exact query.
 * A person's explicit Retry/Refresh still calls that query's refetch directly.
 */
export class LiveQueryClient extends QueryClient {
  constructor(config?: QueryClientConfig) {
    super(config);
    observeReadAttempts(this);
    shellBootstrapIndex(this.getQueryCache());
    resourceRowIndex(this.getQueryCache());
  }

  private reads = new WeakMap<object, { hash: string; original: object; wrapped: object }>();
  override query: QueryClient['query'] = (options) => {
    const queryHash = options.queryHash ?? super.defaultQueryOptions(options).queryHash;
    const reading = super.query({ ...options, queryHash });
    const query = this.getQueryCache().get(queryHash);
    const release = query === undefined || query.state.fetchStatus === 'idle' ? () => undefined : holdAwaitedRead(this, query);
    return reading.finally(release);
  };
  override defaultQueryOptions: QueryClient['defaultQueryOptions'] = (options) => {
    let defaults = super.defaultQueryOptions(options);
    const supplied = startingRead(defaults.queryKey);
    if (supplied !== undefined) defaults = { ...defaults, ...supplied };
    else if (defaults.staleTime === undefined) defaults = { ...defaults, staleTime: () => readFreshness(options.queryKey) };
    if (defaults.placeholderData === undefined && threadReadIdentity(defaults.queryKey) !== null) {
      defaults = { ...defaults, placeholderData: keptThreadRead(this, defaults.queryKey) as typeof defaults.placeholderData };
    }
    const read = defaults.queryFn;
    if (typeof read !== 'function') return defaults;
    const hash = defaults.queryHash;
    const held = this.reads.get(read);
    if (held?.hash === hash && held.wrapped === read) return defaults;
    // The wrapper preserves the native function signature. A copied function used for a
    // different hash gets that query's own cancellation and publication checks.
    let wrapped: typeof read;
    if (held?.hash === hash) wrapped = held.wrapped as typeof read;
    else {
      const original = (held?.original ?? read) as typeof read;
      wrapped = (context) => {
        const reader = client.session.getState();
        return queryRead(this, hash, () => original(context), requestReader(reader.userId, reader.readerVersion));
      };
      const receipt = { hash, original, wrapped };
      this.reads.set(original, receipt);
      this.reads.set(wrapped, receipt);
    }
    return { ...defaults, queryFn: wrapped, ...(defaults.retryDelay === undefined ? {} : { retryDelay: observedRetryDelay(defaults.retryDelay) }) };
  };

  override async invalidateQueries<Key extends QueryKey = QueryKey>(filters: InvalidateQueryFilters<Key> = {}, options?: InvalidateOptions): Promise<void> {
    if (filters.exact !== true) return;
    await super.invalidateQueries(filters, options);
  }
}

export const queryClient = new LiveQueryClient({
  defaultOptions: {
    queries: {
      staleTime: (query) => readFreshness(query.queryKey),
      gcTime: KEPT_FOR_MS,
      retry: retryRead,
      retryDelay: (attempt) => retryPause(attempt),
      // A request is sent whatever the browser believes about the line: a failure is shown where the
      // content would be, with the way to try again, instead of a wait that never ends.
      networkMode: 'always',
      // The socket resumes each visible feed from its last sequence. Focus and a returning
      // network do not make every query ask the server for the same rows again.
      refetchOnReconnect: false,
      refetchOnWindowFocus: false,
    },
    mutations: {
      retry: false,
      networkMode: 'always',
    },
  },
});

setOrchLinkSource(() => queryClient.getQueryData<OrchLinkAddresses>(['meta']));
readFreshnessSettings(readingPreferences(queryClient).freshness);

queryClient.getQueryCache().subscribe((event) => {
  if (event.type === 'removed') {
    const current = queryClient.getQueryCache().get(event.query.queryHash);
    if (current !== undefined && current !== event.query) return;
    const key: unknown = event.query.queryKey;
    if (Array.isArray(key)) scheduledReads.release(key);
  }
});

/** An error as a screen tells it apart: an answer of the API, no answer, or a fault of the page itself. */
export type Failure =
  | { kind: 'api'; error: ApiError }
  | { kind: 'network'; error: NetworkError }
  | { kind: 'defect'; error: unknown };

/** Sorts what a request, a loader or a view threw. */
export function failureOf(error: unknown): Failure {
  if (error instanceof ApiError) return { kind: 'api', error };
  if (error instanceof NetworkError) return { kind: 'network', error };
  return { kind: 'defect', error };
}
