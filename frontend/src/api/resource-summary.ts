/** One native inventory summary, shared by the landing and the resource navigation. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';

export const resourceSummaryKey = ['resource-summary'] as const;
export const resourceSummaryQuery = (opening: string) => queryOptions({
  queryKey: [...resourceSummaryKey, opening] as const,
  // Navigation and the landing share this opening's native count, even if code arrives later.
  staleTime: Infinity,
  refetchOnWindowFocus: false,
  refetchOnReconnect: false,
  queryFn: ({ signal }) => ok(api.GET('/resources/summary', { signal })),
});
