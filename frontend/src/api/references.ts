/** API client for references: the card a reference's chip opens, asked once and kept for the session. */
import { queryOptions } from '@tanstack/react-query';

import { api, ok } from '@/api/client';
import type { TypedReference } from '@/features/people/references';
import type { ReferencePreview } from '@/generated/api-types';

interface ObservedPreview extends ReferencePreview { read_started_at?:number }

/** One fresh attempt after a deadline, including a response whose request began before it but arrived late. */
export function referenceRefreshDelay(data:Pick<ObservedPreview,'refresh_at'|'read_started_at'>|undefined,updatedAt:number,errorAt:number,now:number):number|false {
  const end=data?.refresh_at;
  if(end===undefined||end===null)return false;
  const deadline=Date.parse(end);
  if((data?.read_started_at??updatedAt)>=deadline||errorAt>=deadline)return false;
  return Math.max(1,deadline-now);
}

function pastDeadline(data:ObservedPreview|undefined):boolean {
  const end=data?.refresh_at;
  return end!==undefined&&end!==null&&Date.parse(end)<=Date.now();
}

/** The card of a reference: the thing and the part it names, as the reader may read them. A person's card is asked
 * once and kept for the session; so is this, since what it shows is a glance and not the thing itself. A caller that
 * survives a reader change may supply its reader scope to keep the two answers apart. */
export const referencePreviewQuery = (reference: TypedReference, cacheScope?: string) =>
  queryOptions({
    queryKey: ['reference-preview', reference.kind, reference.id, reference.block ?? null, reference.turn ?? null, reference.message ?? null, reference.path ?? null, reference.line ?? null, ...(cacheScope === undefined ? [] : [cacheScope])] as const,
    queryFn: async ({ signal }): Promise<ObservedPreview> => {
      const readStartedAt=Date.now();
      const preview=await ok(api.GET('/references/preview', {
        params: {
          query: {
            kind: reference.kind,
            id: reference.id,
            ...(reference.block === undefined ? {} : { block: reference.block }),
            ...(reference.turn === undefined ? {} : { turn: reference.turn }),
            ...(reference.message === undefined ? {} : { message: reference.message }),
            ...(reference.path === undefined ? {} : { path: reference.path }),
            ...(reference.line === undefined ? {} : { line: reference.line }),
          },
        },
        signal,
      }));
      return {...preview,read_started_at:readStartedAt};
    },
    staleTime: Infinity,
    refetchIntervalInBackground:true,
    refetchOnMount:query=>pastDeadline(query.state.data)?'always':true,
    refetchOnWindowFocus:query=>pastDeadline(query.state.data)?'always':true,
    refetchOnReconnect:query=>pastDeadline(query.state.data)?'always':true,
    refetchInterval: (query) => referenceRefreshDelay(query.state.data,query.state.dataUpdatedAt,query.state.errorUpdatedAt,Date.now()),
    retry: false,
  });
