/** The retained-storage report shares its filters, figures and exact record page. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import { api, ok } from '@/api/client';
import type { CleanupPlan, CleanupRequest, CleanupStart, Job, StorageQuery, StorageReport } from '@/generated/api-types';

export const storageKeys = { all: ['storage'] as const };

export function planCleanup(body: CleanupRequest): Promise<CleanupPlan> {
  return ok(api.POST('/storage/cleanup/preflight', { body }));
}

export function startCleanup(body: CleanupStart): Promise<Job> {
  return ok(api.POST('/storage/cleanup', { body }));
}

export const storageQuery = (query: StorageQuery) => queryOptions({
  ...(query.scope === 'system' ? ADMIN_QUERY : {}),
  queryKey: [...storageKeys.all, query] as const,
  queryFn: ({ signal }): Promise<StorageReport> => ok(api.GET('/storage', { params: { query }, signal })),
});

export const storagePagesQuery = (query: StorageQuery) => infiniteQueryOptions({
  ...(query.scope === 'system' ? ADMIN_QUERY : {}),
  queryKey: [...storageKeys.all, 'records', query] as const,
  initialPageParam: query.cursor ?? null as string | null,
  queryFn: ({ pageParam, signal }): Promise<StorageReport> => ok(api.GET('/storage', {params:{query:{...query,cursor:pageParam ?? undefined}},signal})),
  getNextPageParam: (page) => page.next_cursor,
});
