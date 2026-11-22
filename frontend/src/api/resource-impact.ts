/** Operation confirmations read the same current dependents and activity as the shared facts service. */
import { queryOptions } from '@tanstack/react-query';
import { api,ok } from '@/api/client';
import type { FactTarget,ResourceImpact,ResourceImpactQuery } from '@/generated/api-types';
import { DEFAULT_RESOURCE_IMPACT_POLL_MS } from '@/lib/pollingDefaults';

/** One second matches the existing job progress cadence without a request per dependent row. */
const IMPACT_REFRESH_MS=DEFAULT_RESOURCE_IMPACT_POLL_MS;

export const resourceImpactQuery=(target:FactTarget,query:ResourceImpactQuery={})=>queryOptions({
  queryKey:query.skill_publication===true?['resource-impact',target,{skill_publication:true}] as const:['resource-impact',target] as const,
  queryFn:({signal}):Promise<ResourceImpact>=>ok(api.POST('/resources/impact',{body:target,params:{query},signal})),
  refetchInterval:IMPACT_REFRESH_MS,
  gcTime:0,
  retry:false,
});
