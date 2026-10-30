/** Active discussion notification work uses bounded pages and the existing job progress contract. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { Job } from '@/generated/api-types';

export const documentNoticeJobKey = (id: string) => ['document-notice-jobs', id] as const;
interface JobPage { items: Job[]; next_cursor: string | null }
interface Answer { data?: JobPage; error?: unknown; response: Response }
type Read = (path: '/documents/{id}/notification-jobs', options: { params: { path: { id: string }; query: { limit: number; cursor?: string } } }) => Promise<Answer>;

export const documentNoticeJobsQuery = (id: string, reader = 0) => infiniteQueryOptions({
  queryKey: [...documentNoticeJobKey(id), reader] as const, initialPageParam: null as string | null, maxPages: 1, gcTime: 0,
  queryFn: ({ pageParam }): Promise<JobPage> => ok((api.GET as unknown as Read)('/documents/{id}/notification-jobs', { params: { path: { id }, query: { limit: 5, ...(pageParam === null ? {} : { cursor: pageParam }) } } })),
  getNextPageParam: (page) => page.next_cursor ?? undefined,
  refetchInterval: (query) => query.state.data?.pages.some((page) => page.items.some((job) => job.state === 'queued' || job.state === 'running')) === true ? 2_000 : false,
});
