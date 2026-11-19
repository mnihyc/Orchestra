/** Operation confirmations read the same current dependents and activity as the shared facts service. */
import { queryOptions } from '@tanstack/react-query';
import { api,ok } from '@/api/client';
import type { FactTarget,ResourceImpact } from '@/generated/api-types';

/** One second matches the existing job progress cadence without a request per dependent row. */
const IMPACT_REFRESH_MS=1_000;

export const resourceImpactQuery=(target:FactTarget)=>queryOptions({
  queryKey:['resource-impact',target] as const,
  queryFn:({signal}):Promise<ResourceImpact>=>ok(api.POST('/resources/impact',{body:target,signal})),
  staleTime:0,
  refetchInterval:IMPACT_REFRESH_MS,
  gcTime:0,
  retry:false,
});
