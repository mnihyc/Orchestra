/** Expanded branches keep their query lifetime while their rows move through the viewport. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useEffect, useMemo, useRef } from 'react';
import { everythingQuery, type EverythingFilter } from '@/api/everything';
import { useListingRetired, useRecentUpdates } from '@/features/navigation/useRecentUpdates';
import { useViewShown } from '@/features/workbench/viewOpening';
import type { Branch } from './rows';

export function BranchReader({ reader, parent, filter, all, publish, clearSelection }: {
  reader: string; parent: string | null; filter: EverythingFilter; all: boolean;
  publish: (parent: string | null, source: Branch | null) => void;
  clearSelection: (() => void) | undefined;
}) {
  const shown = useViewShown();
  const options = everythingQuery(reader, parent, filter);
  const level = useInfiniteQuery({ ...options, enabled: shown });
  const retired = useListingRetired(options.queryKey, level.error);
  const cleared = useRef(false);
  useEffect(() => {
    if (retired && !cleared.current) { cleared.current = true; clearSelection?.(); }
    if (!retired) cleared.current = false;
  }, [retired, clearSelection]);
  useRecentUpdates(options.queryKey, shown && (filter.recent !== undefined || filter.sort === 'opened'), filter.recent === 'opened' || filter.sort === 'opened');
  const rows = useMemo(() => level.data?.pages.flatMap((page) => page.items) ?? [], [level.data]);
  const { hasNextPage, isFetching, isError, fetchNextPage, refetch } = level;
  useEffect(() => { if (shown && all && hasNextPage && !isFetching && !isError) void fetchNextPage(); }, [shown, all, hasNextPage, isFetching, isError, fetchNextPage]);
  const source = useMemo<Branch>(() => ({ rows, pending: level.isPending, error: level.error, retired,
    hasMore: hasNextPage, loadingMore: level.isFetchingNextPage, more: fetchNextPage, retry: refetch,
  }), [rows, level.isPending, level.error, retired, hasNextPage, level.isFetchingNextPage, fetchNextPage, refetch]);
  useEffect(() => {
    let current = true;
    // Publishing outside the commit lets an arbitrarily deep cached hierarchy open without nested React commits.
    queueMicrotask(() => { if (current) publish(parent, source); });
    return () => { current = false; };
  }, [parent, source, publish]);
  useEffect(() => () => { publish(parent, null); }, [parent, publish]);
  return null;
}
