/** One bounded card page supplies both chart modes and follows the provider's observation clock. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from './client';
import type { ProviderPlanSeries, UsageBucket } from '@/generated/api-types';

/** Twenty cards keep the response bounded; further windows are read only when requested. */
export const PLAN_PAGE_SIZE=20;
export interface PlanPage {items:ProviderPlanSeries[];next_cursor:string | null}
export const planUsageQuery=(id:string,from:string,to:string,bucket:UsageBucket,factsAt:string | null,cursor?:string)=>queryOptions({
  // Only a new source clock or an explicit page/span/interval changes this measured snapshot.
  queryKey:['providerPlan',id,from,to,bucket,factsAt,cursor ?? null] as const,
  staleTime:Infinity,
  queryFn:({signal}):Promise<PlanPage>=>ok(api.GET('/providers/{id}/plan',{params:{path:{id},query:{from,to,bucket,limit:PLAN_PAGE_SIZE,cursor:cursor ?? null}},signal})),
});
