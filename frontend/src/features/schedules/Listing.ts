/** The accepted complete topic projection seeds small lists; larger or searched rows use the native pager. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { inputSchedulesPageQuery, topicSchedulesQuery } from '@/api/inputSchedules';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { useRetainedRows } from '@/lib/retainedRows';
import { belongsToThread } from './facts';

export function useScheduleListing(topic: string, thread?: string, enabled = true) {
  const search = useCollectionSearch('schedules:' + topic + ':' + (thread ?? 'all'));
  const snapshot = useQuery({ ...topicSchedulesQuery(topic), enabled });
  const complete = (snapshot.data ?? []).filter((row) => thread === undefined || belongsToThread(row, thread));
  const paged = search.q !== '' || complete.length > 50;
  const pages = useInfiniteQuery({ ...inputSchedulesPageQuery(topic, search.q, thread), enabled: enabled && paged });
  const rows = paged ? pages.data?.pages.flatMap((page) => page.items) ?? [] : complete;
  const held = useRetainedRows(rows);
  return { search, query: paged ? pages : snapshot, pages, paged, rows, held };
}
