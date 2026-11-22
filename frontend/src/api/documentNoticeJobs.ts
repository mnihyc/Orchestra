/** Active discussion notification work uses bounded pages and the existing job progress contract. */
import { infiniteQueryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { Job } from '@/generated/api-types';
import { DEFAULT_DOCUMENT_NOTICE_POLL_MS } from '@/lib/pollingDefaults';

export const documentNoticeJobKey = (id: string) => ['document-notice-jobs', id] as const;
interface JobPage { items: Job[]; next_cursor: string | null }
export const documentNoticeJobsQuery = (id: string, reader = 0,q='') => infiniteQueryOptions({
  queryKey: [...documentNoticeJobKey(id), reader,q] as const, initialPageParam: null as string | null, maxPages: 1, gcTime: 0,
  queryFn: ({ pageParam,signal }): Promise<JobPage> => ok(api.GET('/documents/{id}/notification-jobs', { params: { path: { id }, query: { limit: 50, ...(q===''?{}:{q}), ...(pageParam === null ? {} : { cursor: pageParam }) } },signal })),
  getNextPageParam: (page) => page.next_cursor ?? undefined,
  refetchInterval: (query) => query.state.data?.pages.some((page) => page.items.some((job) => job.state === 'queued' || job.state === 'running')) === true ? DEFAULT_DOCUMENT_NOTICE_POLL_MS : false,
});
