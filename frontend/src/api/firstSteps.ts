/** One read of observed setup facts for the currently chosen org. */
import { queryOptions } from '@tanstack/react-query';

import { ApiError, api, ok } from '@/api/client';
import type { FirstSteps, FirstStepsChoice } from '@/generated/api-types';
export const firstStepsKey = (orgId: string | null) => ['first-steps', orgId] as const;

/** A missing endpoint is a load failure; only a forbidden read is an access refusal. */
export function firstStepsAccessRefused(error:unknown):boolean {return error instanceof ApiError && error.status === 403;}

/** Each opening rechecks authority; a cached refusal can never expose old facts. */
export const firstStepsQuery = (orgId: string | null) => queryOptions({
  queryKey: firstStepsKey(orgId),
  queryFn: ({ signal }): Promise<FirstSteps> => ok(api.GET('/me/first-steps', {
    params: { query: orgId === null ? {} : { org_id: orgId } },
    signal,
  })),
  staleTime: 0,
});

/** Saves just this person's choices; the server preserves unrelated hints and navigation atomically. */
export function setFirstSteps(choice:FirstStepsChoice):Promise<Record<string,unknown>> {
  return ok(api.PUT('/me/first-steps',{body:choice}));
}
