/** Raw frames of an agent segment, paged without loading a whole conversation. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { ThreadRawListReply } from '@/generated/api-types';

/** A frame page is bounded so a noisy harness cannot fill the browser in one read. */
const RAW_PAGE = 100;

export const rawFramesQuery = (thread: string, segment: string) => infiniteQueryOptions({
  queryKey: ['thread-raw', thread, segment] as const,
  initialPageParam: null as number | null,
  queryFn: ({ signal, pageParam }): Promise<ThreadRawListReply> => ok(api.GET('/threads/{id}/raw', {
    params: { path: { id: thread }, query: { segment_id: segment, limit: RAW_PAGE, ...(pageParam === null ? {} : { before: pageParam }) } }, signal,
  })),
  getNextPageParam: (last) => last.has_older ? last.events.at(-1)?.n : undefined,
  retry: false,
});
