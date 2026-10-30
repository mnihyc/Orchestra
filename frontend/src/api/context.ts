/** What fills a thread's context, what each of its turns used, and what one turn was counted at. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import type { ThreadContext, ThreadTurnUsage, TurnCost } from '@/generated/api-types';

/**
 * What fills the context, as kept at the end of the last turn; with `count`, counted by the harness first where it
 * can (an idle thread whose harness breaks its context down), once for each turn. `revision` is the latest change of
 * the thread's turns that the page holds: a turn that ends, or a breakdown that arrives after it, asks again.
 */
export const threadContextQuery = (thread: string, detail: 'kept' | 'count', revision: number) => queryOptions({
  queryKey: ['thread-context', thread, detail, revision] as const,
  queryFn: ({ signal }): Promise<ThreadContext> =>
    ok(api.GET('/threads/{id}/context', { params: { path: { id: thread }, query: detail === 'count' ? { detail: 'count' } : {} }, signal })),
  retry: false,
});

/** The thread's turns with what each used, the latest page first and older ones on request. */
export const threadTurnUsageQuery = (thread: string, revision: number) => infiniteQueryOptions({
  queryKey: ['thread-turn-usage', thread, revision] as const,
  queryFn: ({ signal, pageParam }): Promise<ThreadTurnUsage> =>
    ok(api.GET('/threads/{id}/usage/turns', { params: { path: { id: thread }, query: pageParam === null ? {} : { before: pageParam } }, signal })),
  initialPageParam: null as number | null,
  getNextPageParam: (page: ThreadTurnUsage) => page.next_before ?? undefined,
  retry: false,
});

/** What one turn was counted at in the ledger, by model. */
export const turnCostQuery = (thread: string, turn: string, revision: number) => queryOptions({
  queryKey: ['turn-cost', thread, turn, revision] as const,
  queryFn: ({ signal }): Promise<TurnCost> =>
    ok(api.GET('/threads/{id}/turns/{turn_id}/cost', { params: { path: { id: thread, turn_id: turn } }, signal })),
  retry: false,
});
