/** One bounded page supplies provider observations; every metric follows the same source clock. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from './client';
import type { ProviderCreditSeries, ProviderHistorySeries, ProviderPlanSeries, UsageBucket } from '@/generated/api-types';

/** Twenty cards keep the response bounded; further series are read only when requested. */
export const PLAN_PAGE_SIZE=20;
export interface PlanPage {items:ProviderPlanSeries[];next_cursor:string | null}
export interface CreditPage {items:ProviderCreditSeries[];next_cursor:string | null}

function isSubscription(series:ProviderHistorySeries):series is ProviderPlanSeries { return 'window_id' in series; }
function isCredit(series:unknown):series is ProviderCreditSeries { return typeof series==='object' && series!==null && 'kind' in series && series.kind==='credits'; }

export const planUsageQuery=(id:string,from:string,to:string,bucket:UsageBucket,factsAt:string | null,cursor?:string)=>queryOptions({
  // Only a new source clock or an explicit page/span/interval changes this measured snapshot.
  queryKey:['providerPlan',id,from,to,bucket,factsAt,cursor ?? null] as const,
  staleTime:Infinity,
  queryFn:async({signal}):Promise<PlanPage>=>{
    const page=await ok(api.GET('/providers/{id}/plan',{params:{path:{id},query:{from,to,bucket,limit:PLAN_PAGE_SIZE,cursor:cursor ?? null}},signal}));
    if(!page.items.every(isSubscription))throw new Error('The provider history response does not match the requested subscription metric');
    return {...page,items:page.items};
  },
});

export const creditUsageQuery=(id:string,from:string,to:string,bucket:UsageBucket,factsAt:string | null,cursor?:string)=>queryOptions({
  queryKey:['providerPlan',id,from,to,bucket,factsAt,cursor ?? null,'credits'] as const,
  staleTime:Infinity,
  queryFn:async({signal}):Promise<CreditPage>=>{
    const page=await ok(api.GET('/providers/{id}/plan',{params:{path:{id},query:{from,to,bucket,metric:'credits',limit:PLAN_PAGE_SIZE,cursor:cursor ?? null}},signal}));
    if(!page.items.every(isCredit))throw new Error('The provider history response does not match the requested credit metric');
    return {...page,items:page.items};
  },
});
