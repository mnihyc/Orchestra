/** Bounded current native pool choices join the consuming resource control's existing groups. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { everythingQuery } from '@/api/everything';
import type { PoolKind } from '@/generated/api-types';
import { resourceChoice } from '@/features/resource-facts/ResourceFacts';
import { useDebounced } from '@/lib/debounce';
import { holds } from '@/lib/permissions';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';

export function usePoolTargetChoices(kind: PoolKind, open: boolean, query: string) {
  const { t } = useTranslation();
  const reader = useStore(client.session, (state)=>`${state.userId ?? ''}:${state.readerVersion ?? 0}`);
  const settled = useDebounced(query.trim());
  const waiting = settled !== query.trim();
  const list = useInfiniteQuery({ ...everythingQuery(reader,'resources',{scope:'mine',kinds:['pool'],q:settled,owner:null,createdFrom:'',createdTo:'',sort:'name',direction:'asc',resource:{view:'usable',details:true,kind}},'resource-target-pools'),enabled:open && !waiting,retry:false });
  const rows = waiting || list.isError && !list.isFetchNextPageError ? [] : [...new Map((list.data?.pages.flatMap((page)=>page.items) ?? []).flatMap((item)=>{
    const pool=item.resource?.kind==='pool'?item.resource.value:undefined;
    return pool!==undefined && pool.kind===kind && item.name!==null && item.my_access!=='path_only' && holds(pool.access,'pool.view') && holds(pool.access,'pool.use') ? [[pool.id,pool] as const] : [];
  })).values()];
  return {rows,loading:waiting || list.isFetching,options:rows.map((pool)=>({value:`\u0000pool:${pool.id}`,label:pool.name,text:pool.name,...resourceChoice({kind:'pool',pool}),disabled:!pool.enabled || pool.policy===null,...(!pool.enabled?{description:t('pools.off')}:pool.policy===null?{description:t('pools.policy.choose')}:{})})),
    footer:<>{list.isError && <ErrorState compact error={list.error} onRetry={()=>void list.refetch()} />}<Pagination hasMore={list.hasNextPage} loading={list.isFetching} onMore={()=>void list.fetchNextPage()} /></>};
}
