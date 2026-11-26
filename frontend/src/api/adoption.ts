/** The sessions a harness already holds, what adopting one would do before anything does, and the adopting itself. */
import { CancelledError, infiniteQueryOptions, queryOptions } from '@tanstack/react-query';

import { ApiError, api, ok, repeatable } from '@/api/client';
import type { AdoptPlan, Thread, ThreadAdoptBody, AdoptHistoryStatus, NativeSessionPage } from '@/generated/api-types';

/** The sessions the harness of a provider and an agent profile holds, each with the thread that adopted it. */
export const harnessSessionsKey = (providerId: string, agentId: string) => ['harness-sessions', providerId, agentId] as const;
export interface NativeSessionRead {
  view: string;
  generation: number;
  source: string | null;
  captured: (source: string) => void;
  expired: () => void;
  refused: (error: unknown) => void;
}
interface NativeContinuation { source: string; cursor: string }
export const harnessSessionsPageQuery = (providerId: string, agentId: string, q: string, read: NativeSessionRead) => infiniteQueryOptions({
  queryKey: [...harnessSessionsKey(providerId, agentId), read.view, read.generation, q] as const,
  initialPageParam: null as NativeContinuation | null,
  queryFn: async ({ pageParam, signal }): Promise<NativeSessionPage> => {
    const source = pageParam?.source ?? read.source;
    try {
      const page = await ok(api.GET('/providers/{id}/harnesses/{agent_id}/sessions', { params: {
        path: { id: providerId, agent_id: agentId }, query: { q, limit: 50, ...(source === null ? {} : { source }), ...(pageParam === null ? {} : { cursor: pageParam.cursor }) },
      }, signal }));
      signal.throwIfAborted(); read.captured(page.source); return page;
    } catch (error) {
      signal.throwIfAborted();
      if (error instanceof ApiError && error.is('request.invalid') && error.faults().some((fault) => fault.path === 'source')) {
        read.expired(); throw new CancelledError({ silent: true });
      }
      if (error instanceof ApiError && [401, 403, 404].includes(error.status)) read.refused(error);
      throw error;
    }
  },
  getNextPageParam: (last) => last.next_cursor === null ? null : { source: last.source, cursor: last.next_cursor },
  retry: false, retryOnMount: false, refetchOnMount: false,
});

/** What adopting the session into the topic would do: where it would stand, the model, the history it brings. */
export const adoptionPlanQuery = (topicId: string, body: ThreadAdoptBody) =>
  queryOptions({
    queryKey: ['adoption-plan', topicId, body] as const,
    queryFn: ({ signal }): Promise<AdoptPlan> =>
      ok(api.POST('/topics/{id}/threads/adopt/preview', { params: { path: { id: topicId } }, body, signal })),
    retry: false,
  });

/** Takes the session in as a thread of the topic, once for `key`. */
export function adoptSession(topicId: string, body: ThreadAdoptBody, key: string): Promise<Thread> {
  return ok(api.POST('/topics/{id}/threads/adopt', { params: { path: { id: topicId } }, body, ...repeatable(key) }));
}

/** Reads a failed adoption's earlier record again, without an agent input or turn. */
export async function retryAdoptedHistory(threadId: string, key: string): Promise<Thread> {
  const result = await ok(api.POST('/threads/{id}/adopt/retry-history', { params: { path: { id: threadId } }, ...repeatable(key) }));
  if ('projection' in result) throw new Error('The history retry completed, but its content is no longer available.');
  return result;
}

/** The band reads only stored history and present machine facts; opening it asks no harness or model. */
export const adoptedHistoryQuery = (threadId: string) => queryOptions({
  queryKey: ['adopt-history', threadId] as const,
  queryFn: ({ signal }): Promise<AdoptHistoryStatus | null> => ok(api.GET('/threads/{id}/adopt/history', { params: { path: { id: threadId } }, signal })),
  retry: false,
});
