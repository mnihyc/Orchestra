/** Executor and tool-set declarations supplied by this build. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { ExecutorInfo } from '@/generated/api-types';

export const executorCatalogueQuery = queryOptions({
  queryKey: ['harness-executors'],
  queryFn: ({ signal }): Promise<ExecutorInfo[]> => ok(api.GET('/harness-executors', { signal })),
});
