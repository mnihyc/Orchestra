/** Immutable program builds, current unit observations and explicit vendor-version requests. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import type { HarnessVersionFetchBody } from '@/generated/api-types';

/** A check is followed only while a visible build is being checked. */
export const BUILD_CHECK_FOLLOW_MS = 5_000;
export const programBuildsQuery = queryOptions({
  queryKey: ['program-builds'],
  queryFn: ({ signal }) => ok(api.GET('/harnesses/builds', { signal })),
  refetchInterval: (query) => query.state.data?.some((build) => build.status === 'checking') === true ? BUILD_CHECK_FOLLOW_MS : false,
});
export const harnessUsageQuery = queryOptions({
  ...ADMIN_QUERY, queryKey: ['admin', 'harness-usage'],
  queryFn: ({ signal }) => ok(api.GET('/admin/harnesses/usage', { signal })),
});
export const environmentProgramsQuery = (id: string, generation?: number) => queryOptions({
  queryKey: ['environment-programs', id, generation ?? null],
  queryFn: ({ signal }) => ok(api.GET('/environments/{id}/harnesses', { params: { path: { id } }, signal })),
  refetchInterval: (query) => query.state.data?.some((program) => program.build?.status === 'checking') === true ? BUILD_CHECK_FOLLOW_MS : false,
});
export function fetchProgramVersion(kind: string, version: string, body: HarnessVersionFetchBody) {
  return ok(api.POST('/harnesses/{kind}/{version}/fetch', { params: { path: { kind, version } }, body }));
}
