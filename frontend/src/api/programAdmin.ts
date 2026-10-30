/** Bounded administrative program pages and mutations that return only the changed build. */
import { queryOptions } from '@tanstack/react-query';
import { ADMIN_PAGE_SIZE } from '@/api/admin';
import { api, ok } from '@/api/client';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import type { HarnessArch, HarnessBuild, HarnessBuildStatus, MachineOs, ProgramDetail, ProgramSummary } from '@/generated/api-types';

export const adminProgramsKey = ['admin', 'programs'] as const;
export interface ProgramPage<Item> { items: Item[]; next_cursor?: string | null }
export interface ProgramBuildFilters { os?: MachineOs; arch?: HarnessArch; status?: HarnessBuildStatus }

/** Leaving a page releases its rows; the cursor in the address is enough to return to it. */
export const adminProgramsQuery = (q: string, cursor: string | undefined) => queryOptions({
  ...ADMIN_QUERY, queryKey: [...adminProgramsKey, 'list', q, cursor ?? null], gcTime: 0,
  queryFn: ({ signal }): Promise<ProgramPage<ProgramSummary>> => ok(api.GET('/admin/programs', {
    params: { query: { limit: ADMIN_PAGE_SIZE, q: q === '' ? null : q, cursor: cursor ?? null } }, signal,
  })),
});
export const adminProgramQuery = (kind: string) => queryOptions({
  ...ADMIN_QUERY, queryKey: [...adminProgramsKey, kind, 'detail'], gcTime: 0,
  queryFn: ({ signal }): Promise<ProgramDetail> => ok(api.GET('/admin/programs/{kind}', { params: { path: { kind } }, signal })),
});
export const adminProgramBuildsQuery = (kind: string, cursor: string | undefined, filters: ProgramBuildFilters = {}) => queryOptions({
  ...ADMIN_QUERY, queryKey: [...adminProgramsKey, kind, 'builds', cursor ?? null, filters], gcTime: 0,
  queryFn: ({ signal }): Promise<ProgramPage<HarnessBuild>> => ok(api.GET('/admin/programs/{kind}/builds', {
    params: { path: { kind }, query: { limit: ADMIN_PAGE_SIZE, cursor: cursor ?? null,
      os: filters.os ?? null, arch: filters.arch ?? null, status: filters.status ?? null } }, signal,
  })),
});
export function saveProgramBuild(build: HarnessBuild): Promise<HarnessBuild> {
  return ok(api.PUT('/admin/programs/{kind}/builds/{version}', { params: { path: { kind: build.kind, version: build.version } }, body: build }));
}
export function checkProgramBuild(build: HarnessBuild) {
  return ok(api.POST('/admin/programs/{kind}/builds/{version}/check', { params: { path: { kind: build.kind, version: build.version } } }));
}
/** An exact version has at most the declared operating-system and architecture combinations. */
export const adminProgramVersionQuery = (kind: string, version: string) => queryOptions({
  ...ADMIN_QUERY, queryKey: [...adminProgramsKey, kind, 'version', version], gcTime: 0,
  queryFn: ({ signal }): Promise<HarnessBuild[]> => ok(api.GET('/admin/programs/{kind}/builds/{version}', { params: { path: { kind, version } }, signal })),
});
export function fetchAdminProgramVersion(kind: string, version: string): Promise<HarnessBuild[]> {
  return ok(api.POST('/admin/programs/{kind}/builds/{version}/fetch', { params: { path: { kind, version } } }));
}
export function readProgramChannels(kind: string) {
  return ok(api.POST('/admin/programs/{kind}/channels', { params: { path: { kind } } }));
}
