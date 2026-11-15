/** Pool metadata and member writes retain exact identity, revision and acknowledgement. */
import { applyRow } from '@/api/cache';
import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { api, ok, repeatable } from '@/api/client';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { resourceRows } from '@/api/listed';
import type { Pool, PoolCreateBody, PoolMemberPutBody, PoolMembersQuery, PoolReceipt, PoolUpdateBody, PoolLendingScope, PoolLendingPutBody, PoolMemberMetadataBody, LendingMatcher, NameHit } from '@/generated/api-types';

export const poolsKey = ['pools'] as const;
export const poolsQuery = queryOptions({ queryKey: poolsKey,
  queryFn: ({ signal }) => ok(api.GET('/pools', { signal })).then(resourceRows<Pool>) });
export const poolQuery = (id: string) => queryOptions({ queryKey: ['pool', id] as const,
  queryFn: ({ signal }): Promise<Pool> => ok(api.GET('/pools/{id}', { params: { path: { id } }, signal })) });
export const poolMembersQuery = (id: string, after: PoolMembersQuery = { after_id: null, after_position: null }) => queryOptions({
  queryKey: ['pool-members', id, after] as const,
  queryFn: ({ signal }) => ok(api.GET('/pools/{id}/members', { params: { path: { id }, query: after }, signal })) });
export const createPool = (body: PoolCreateBody, key: string): Promise<PoolReceipt> =>
  ok(api.POST('/pools', { body, ...repeatable(key) }));
export const updatePool = (id: string, body: PoolUpdateBody): Promise<PoolReceipt> =>
  ok(api.PUT('/pools/{id}', { params: { path: { id } }, body }));
export const putPoolMember = (id: string, body: PoolMemberPutBody): Promise<PoolReceipt> =>
  ok(api.PUT('/pools/{id}/members/{member_id}', { params: { path: { id, member_id: body.member.id } }, body }));

export const poolLendingQuery = (scope: PoolLendingScope, after: string | null = null) => queryOptions({
  queryKey: ['pool-lending', scope, after] as const,
  queryFn: ({ signal }) => ok(api.POST('/pools/lending/list', { body: { scope, after_id: after }, signal })),
});
export const putPoolLending = (id: string, body: PoolLendingPutBody) => ok(api.PUT('/pools/lending/{entry_id}', { params: { path: { entry_id: id } }, body }));
export const poolExpandedQuery = (id: string, cursor: string | null) => queryOptions({ queryKey: ['pool-expanded', id, cursor] as const,
  queryFn: ({ signal }) => ok(api.GET('/pools/{id}/expanded', { params: { path: { id }, query: { cursor } }, signal })) });
export const removePoolMember = (id: string, member_id: string, expected_revision: number) =>
  ok(api.DELETE('/pools/{id}/members/{member_id}', { params: { path: { id, member_id }, query: { expected_revision } } }));

/** A confirmed write is followed by this read only; it never resubmits the mutation. */
export async function readPoolAfterWrite(cache: QueryClient, id: string): Promise<Pool> {
  const reader=libraryReader();
  const pool=await ok(api.GET('/pools/{id}', {params:{path:{id}}}));
  if (!sameLibraryReader(reader)) throw new DOMException('The requesting reader changed.', 'AbortError');
  applyRow(cache,{kind:'pool',id,value:{...pool},insert:true});
  cache.setQueryData(poolQuery(id).queryKey,pool);
  return pool;
}
export const putPoolMemberMetadata = (id: string, member_id: string, body: PoolMemberMetadataBody) =>
  ok(api.PATCH('/pools/{id}/members/{member_id}', { params: { path: { id, member_id } }, body }));
export const deletePool = (id: string, expected_revision: number, key: string) =>
  ok(api.DELETE('/pools/{id}', { params: { path: { id } }, body: { expected_revision }, ...repeatable(key) }));

/** Resolve only the authored targets through the ordinary current name boundary. */
export const poolConsentNamesQuery = (matcher: LendingMatcher) => queryOptions({ queryKey: ['pool-consent-names', matcher] as const, retry: false,
  queryFn: async ({signal}):Promise<NameHit[]> => {
    const reader=libraryReader();
    const ids=matcher.kind === 'managers' ? matcher.manager_ids : [matcher.kind === 'pool' ? matcher.pool_id : matcher.home_id];
    const kinds=matcher.kind === 'pool' ? 'pool' : matcher.kind === 'home' ? 'org,project' : 'user';
    const names:NameHit[]=[];
    for(const id of ids) {
      const page=await ok(api.GET('/search/names',{params:{query:{id,kinds}},signal}));
      names.push(...page.items.filter((row)=>row.id===id));
    }
    if (!sameLibraryReader(reader)) throw new DOMException('The requesting reader changed.', 'AbortError');
    return names;
  },
});
