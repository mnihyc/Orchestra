/** Pool metadata and member writes retain exact identity, revision and acknowledgement. Every pool write answers
 * with its receipt (id and revision, never the pool's content), which the pool page can use immediately. */
import { applyRow } from '@/api/cache';
import { queryOptions, type QueryClient } from '@tanstack/react-query';
import { accepted, api, ok, repeatable } from '@/api/client';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { resourceRows } from '@/api/listed';
import { poolReadCut, samePoolReadCut } from '@/api/poolReadCut';
import type { Pool, PoolCreateBody, PoolDuplicateBody, PoolMemberPutBody, PoolMembersQuery, PoolChoicesQuery, PoolReceipt, PoolUpdateBody, PoolLendingScope, PoolLendingPutBody, PoolMemberMetadataBody, LendingMatcher, NameHit } from '@/generated/api-types';

export const poolsKey = ['pools'] as const;
const poolReader=()=>{const reader=libraryReader();return `${reader[0] ?? ''}:${reader[1] ?? 0}`;};
async function readPool<T>(reader:string, read:()=>Promise<T>):Promise<T> {
  const cut=poolReadCut();
  if (reader!==poolReader()) throw new DOMException('The requesting reader changed.', 'AbortError');
  const value=await read();
  if (reader!==poolReader() || !samePoolReadCut(cut)) throw new DOMException('The requesting reader changed.', 'AbortError');
  return value;
}
export const poolsQuery = (reader=poolReader()) => queryOptions({ queryKey: [...poolsKey,reader] as const,
  queryFn: ({ signal }) => readPool(reader,()=>ok(api.GET('/pools', { signal })).then(resourceRows<Pool>)) });
export const poolQuery = (id: string, reader=poolReader()) => queryOptions({ queryKey: ['pool', id, reader] as const,
  queryFn: ({ signal }): Promise<Pool> => readPool(reader,()=>ok(api.GET('/pools/{id}', { params: { path: { id } }, signal }))) });
export const poolMembersQuery = (id: string, after: PoolMembersQuery = { after_id: null, after_position: null }, reader=poolReader()) => queryOptions({
  queryKey: ['pool-members', id, after,reader] as const,
  queryFn: ({ signal }) => readPool(reader,()=>ok(api.GET('/pools/{id}/members', { params: { path: { id }, query: after }, signal }))) });
/** Answered as soon as the server accepts it: New pool closes on it and opens the pool. */
export const createPool = (body: PoolCreateBody, key: string): Promise<PoolReceipt> =>
  accepted(api.POST('/pools', { body, ...repeatable(key, { answerFirst: true }) }));
export const duplicatePool = (id: string, body: PoolDuplicateBody, key: string): Promise<PoolReceipt> =>
  accepted(api.POST('/pools/{id}/duplicate', { params: { path: { id } }, body, ...repeatable(key) }));
export const updatePool = (id: string, body: PoolUpdateBody): Promise<PoolReceipt> =>
  accepted(api.PUT('/pools/{id}', { params: { path: { id } }, body }));
export const putPoolMember = (id: string, body: PoolMemberPutBody): Promise<PoolReceipt> =>
  accepted(api.PUT('/pools/{id}/members/{member_id}', { params: { path: { id, member_id: body.member.id } }, body }));

export const poolLendingQuery = (scope: PoolLendingScope, after: string | null = null,reader=poolReader()) => queryOptions({
  queryKey: ['pool-lending', scope, after,reader] as const,
  queryFn: ({ signal }) => readPool(reader,()=>ok(api.POST('/pools/lending/list', { body: { scope, after_id: after }, signal }))),
});
export const putPoolLending = (id: string, body: PoolLendingPutBody): Promise<PoolReceipt> => accepted(api.PUT('/pools/lending/{entry_id}', { params: { path: { entry_id: id } }, body }));
export const poolExpandedQuery = (id: string, cursor: string | null,reader=poolReader()) => queryOptions({ queryKey: ['pool-expanded', id, cursor,reader] as const,
  queryFn: ({ signal }) => readPool(reader,()=>ok(api.GET('/pools/{id}/expanded', { params: { path: { id }, query: { cursor } }, signal }))) });
export const poolChoicesQuery = (id:string, before:PoolChoicesQuery,reader=poolReader()) => queryOptions({queryKey:['pool-choices',id,before,reader] as const,
  queryFn:({signal})=>readPool(reader,()=>ok(api.GET('/pools/{id}/choices',{params:{path:{id},query:before},signal}))) });
export const poolChoiceDetailsQuery = (id:string,choice_id:string,after_ordinal:number|null,reader=poolReader())=>queryOptions({queryKey:['pool-choice-details',id,choice_id,after_ordinal,reader] as const,
  queryFn:({signal})=>readPool(reader,()=>ok(api.GET('/pools/{id}/choices/{choice_id}',{params:{path:{id,choice_id},query:{after_ordinal}},signal}))) });
export const removePoolMember = (id: string, member_id: string, expected_revision: number): Promise<PoolReceipt> =>
  accepted(api.DELETE('/pools/{id}/members/{member_id}', { params: { path: { id, member_id }, query: { expected_revision } } }));

/** A new pool shares the page's first read. An edit needs a fresh response after its write. */
export async function readPoolAfterWrite(cache: QueryClient, id: string, change: 'created' | 'changed' = 'changed'): Promise<Pool> {
  const reader=libraryReader();
  const cut=poolReadCut();
  const query = poolQuery(id);
  const pool = change === 'created' ? await cache.query(query) : await ok(api.GET('/pools/{id}', {params:{path:{id}}}));
  if (!sameLibraryReader(reader) || !samePoolReadCut(cut)) throw new DOMException('The requesting reader changed.', 'AbortError');
  applyRow(cache,{kind:'pool',id,value:{...pool},insert:true});
  if (change === 'changed') cache.setQueryData(query.queryKey, pool);
  return pool;
}
export const putPoolMemberMetadata = (id: string, member_id: string, body: PoolMemberMetadataBody): Promise<PoolReceipt> =>
  accepted(api.PATCH('/pools/{id}/members/{member_id}', { params: { path: { id, member_id } }, body }));
export const deletePool = (id: string, expected_revision: number, key: string) =>
  accepted(api.DELETE('/pools/{id}', { params: { path: { id } }, body: { expected_revision }, ...repeatable(key) }));
export const recoverPool = (id: string, expected_revision: number, key: string): Promise<PoolReceipt> =>
  accepted(api.POST('/pools/{id}/recover', { params: { path: { id } }, body: { expected_revision }, ...repeatable(key) }));

/** Resolve only the authored targets through the ordinary current name boundary. */
export const poolConsentNamesQuery = (matcher: LendingMatcher,reader=poolReader()) => queryOptions({ queryKey: ['pool-consent-names', matcher,reader] as const, retry: false,
  queryFn: ({signal}):Promise<NameHit[]> => readPool(reader,async()=>{
    const ids=matcher.kind === 'managers' ? matcher.manager_ids : [matcher.kind === 'pool' ? matcher.pool_id : matcher.home_id];
    const kinds=matcher.kind === 'pool' ? 'pool' : matcher.kind === 'home' ? 'org,project' : 'user';
    const names:NameHit[]=[];
    for(const id of ids) {
      const page=await ok(api.GET('/search/names',{params:{query:{id,kinds}},signal}));
      names.push(...page.items.filter((row)=>row.id===id));
    }
    return names;
  }),
});
