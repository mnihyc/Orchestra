/** Accepted administrator messages and the current native sent-list audience preview. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, accepted, ok, repeatable } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { ADMIN_PAGE_SIZE } from '@/api/admin';
import type { AdminMessage, AdminMessageCreateBody, AdminMessageCreateResponse, AdminMessageListResponse } from '@/generated/api-types';

export const sentMessagesKey = (org: string | null) => ['admin-messages', org ?? 'system'] as const;
export const sentMessagesQuery = (org: string | null) => infiniteQueryOptions({
  ...(org === null ? ADMIN_QUERY : {}),
  queryKey: sentMessagesKey(org),
  initialPageParam: null as string | null,
  queryFn: ({ pageParam, signal }): Promise<AdminMessageListResponse> => org === null
    ? ok(api.GET('/admin/messages', { params: { query: { limit: ADMIN_PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal }))
    : ok(api.GET('/orgs/{id}/messages', { params: { path: { id: org }, query: { limit: ADMIN_PAGE_SIZE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal })),
  getNextPageParam: (page) => page.next_cursor,
});

export const messageQuery = (id: string) => queryOptions({
  queryKey: ['admin-message', id] as const,
  queryFn: ({ signal }): Promise<AdminMessage> => ok(api.GET('/messages/{id}', { params: { path: { id } }, signal })),
});

export function sendMessage(org: string | null, body: AdminMessageCreateBody, key: string): Promise<AdminMessageCreateResponse> {
  return org === null
    ? accepted(api.POST('/admin/messages', { body, ...repeatable(key) }))
    : accepted(api.POST('/orgs/{id}/messages', { params: { path: { id: org } }, body, ...repeatable(key) }));
}
