/** A native opening supplies each independent section's exact existing query. */
import type { QueryClient, QueryKey } from '@tanstack/react-query';
import { ApiError } from '@/api/client';
import type { Problem } from '@/generated/api-types';

export type NativeOverviewPart<T> = { status: 'ready'; data: T }
  | { status: 'failed'; problem: Problem; retry_after?: number | null } | { status: 'omitted' };

export function supplyOverviewPart<T>(cache: QueryClient, key: QueryKey, part: NativeOverviewPart<T>, project: (value: T) => unknown = (value) => value): boolean {
  if (part.status === 'omitted') return false;
  if (part.status === 'ready') cache.setQueryData(key, project(part.data));
  else cache.getQueryCache().build(cache, { queryKey: key }).setState({ data: undefined, status: 'error', fetchStatus: 'idle',
    error: new ApiError(part.problem, part.retry_after === null || part.retry_after === undefined ? null : new Date(Date.now() + part.retry_after * 1_000)) });
  return true;
}
