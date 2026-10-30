/** What a list of Links shows of the pages read so far, whatever query read them. */
import type { LinkedThing } from '@/generated/api-types';

/** The part of an infinite query a list draws from. */
export interface PagedQuery {
  data: { pages: readonly { items: LinkedThing[] }[] } | undefined;
  isPending: boolean;
  isError: boolean;
  error: unknown;
  hasNextPage: boolean;
  isFetchingNextPage: boolean;
  isFetchNextPageError: boolean;
  fetchNextPage: (options?: { cancelRefetch?: boolean }) => Promise<unknown>;
  refetch: () => Promise<unknown>;
}

export interface ListState {
  rows: readonly LinkedThing[];
  /** The first page has not come. */
  pending: boolean;
  /** The first page failed. */
  failed: boolean;
  error: unknown;
  hasMore: boolean;
  fetching: boolean;
  /** The next page failed, with the rows before it kept. */
  moreFailed: boolean;
  more: () => void;
  retry: () => void;
}

export function listState(query: PagedQuery): ListState {
  const rows = query.data?.pages.flatMap((page) => page.items) ?? [];
  return {
    rows,
    pending: query.isPending,
    failed: query.isError && rows.length === 0,
    error: query.error,
    hasMore: query.hasNextPage,
    fetching: query.isFetchingNextPage,
    moreFailed: query.isFetchNextPageError,
    // The button and scroll observer may fire before React renders loading: share that page's request.
    more: () => void query.fetchNextPage({ cancelRefetch: false }),
    retry: () => void query.refetch(),
  };
}
