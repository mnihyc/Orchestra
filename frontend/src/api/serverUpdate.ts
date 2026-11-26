/** The server update stays a normal durable job; only its bundled node runs the existing updater. */
import { queryOptions } from '@tanstack/react-query';
import { accepted, api, ApiError, ok } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import type { Job, ServerUpdateView } from '@/generated/api-types';
import { DEFAULT_INSTALLATION_POLL_SECONDS } from '@/lib/pollingDefaults';

export const serverUpdateKey = ['admin', 'server-update'] as const;
export const serverUpdateOpen = (job: Job | null | undefined): boolean => job?.state === 'queued' || job?.state === 'running';
/** Authorization and permanent read failures are not a server restart. */
export function serverRestartError(error: unknown): boolean {
  return error instanceof ApiError ? [0, 502, 503, 504].includes(error.status) : error instanceof TypeError;
}
export const serverUpdateQuery = queryOptions({ ...ADMIN_QUERY, queryKey: serverUpdateKey,
  queryFn: ({ signal }): Promise<ServerUpdateView> => ok(api.GET('/admin/server-update', { signal })),
  refetchInterval: (query) => serverUpdateOpen(query.state.data?.job) || serverRestartError(query.state.error) ? DEFAULT_INSTALLATION_POLL_SECONDS * 1000 : false,
});
export function startServerUpdate(release: string): Promise<Job> {
  return accepted(api.POST('/admin/server-update', { body: { release } }));
}
