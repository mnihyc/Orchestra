/** The sessions a harness already holds, what adopting one would do before anything does, and the adopting itself. */
import { queryOptions } from '@tanstack/react-query';

import { api, ok, repeatable } from '@/api/client';
import type { AdoptPlan, Thread, ThreadAdoptBody, AdoptHistoryStatus } from '@/generated/api-types';

/** The sessions the harness of a provider and an agent profile holds, each with the thread that adopted it. */
export const harnessSessionsQuery = (providerId: string, agentId: string) =>
  queryOptions({
    queryKey: ['harness-sessions', providerId, agentId] as const,
    queryFn: ({ signal }) =>
      ok(api.GET('/providers/{id}/harnesses/{agent_id}/sessions', { params: { path: { id: providerId, agent_id: agentId } }, signal })),
    // A harness that cannot list its sessions says so once; asking again changes nothing.
    retry: false,
  });

/** What adopting the session into the topic would do: where it would stand, the model, the history it brings. */
export const adoptionPlanQuery = (topicId: string, body: ThreadAdoptBody) =>
  queryOptions({
    queryKey: ['adoption-plan', topicId, body.provider_id, body.agent_id, body.native_session_ref] as const,
    queryFn: ({ signal }): Promise<AdoptPlan> =>
      ok(api.POST('/topics/{id}/threads/adopt/preview', { params: { path: { id: topicId } }, body, signal })),
    staleTime: 0,
    retry: false,
  });

/** Takes the session in as a thread of the topic, once for `key`. */
export function adoptSession(topicId: string, body: ThreadAdoptBody, key: string): Promise<Thread> {
  return ok(api.POST('/topics/{id}/threads/adopt', { params: { path: { id: topicId } }, body, ...repeatable(key) }));
}

/** Reads a failed adoption's earlier record again, without an agent input or turn. */
export function retryAdoptedHistory(threadId: string, key: string): Promise<Thread> {
  return ok(api.POST('/threads/{id}/adopt/retry-history', { params: { path: { id: threadId } }, ...repeatable(key) }));
}

/** The band reads only stored history and present machine facts; opening it asks no harness or model. */
export const adoptedHistoryQuery = (threadId: string) => queryOptions({
  queryKey: ['adopt-history', threadId] as const,
  queryFn: ({ signal }): Promise<AdoptHistoryStatus | null> => ok(api.GET('/threads/{id}/adopt/history', { params: { path: { id: threadId } }, signal })),
  retry: false,
});
