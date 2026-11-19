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
import { QueryClient, type InvalidateOptions, type InvalidateQueryFilters, type QueryKey } from '@tanstack/react-query';

import { ApiError } from '@/api/client';
import { NetworkError, retryPause, retryRead } from '@/lib/requests';

/**
 * How long an answer counts as fresh. Within it a screen that is opened again shows what it has and
 * asks nothing; after it the screen shows what it has and asks behind it. Half a minute covers going
 * back and forth between two screens without a request each time, and nothing a person acts on is
 * read from here alone: what changes while they look arrives by the live connection.
 */
const FRESH_FOR_MS = 30_000;

/** How long an answer nobody shows is kept: long enough to come back to a screen and find it there. */
const KEPT_FOR_MS = 5 * 60_000;

/** A passing inspection may become kept while its one detail read finishes into the same cache. */
export interface ReadOptions {
  keepReading?: boolean;
}

/** Legacy screen callbacks cannot turn one act into a refresh of every list of its kind.
 * Native act/feed answers patch the cache; a missing projection must name one exact query.
 * A person's explicit Retry/Refresh still calls that query's refetch directly.
 */
export class LiveQueryClient extends QueryClient {
  override async invalidateQueries<Key extends QueryKey = QueryKey>(filters: InvalidateQueryFilters<Key> = {}, options?: InvalidateOptions): Promise<void> {
    if (filters.exact !== true) return;
    await super.invalidateQueries(filters, options);
  }
}

export const queryClient = new LiveQueryClient({
  defaultOptions: {
    queries: {
      staleTime: FRESH_FOR_MS,
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
