/** API client for Fork and switch plans with their options, single and batch.
 *
 * Besides the plans: the job that carries one out, the places a new environment can be made at, and side threads.
 * What the wizard of a fork or a switch asks of the API: the plan of one thread (`ForkRequest`, and what it
 * would do before anything does), the plan of several threads switched together, carrying a plan out, the job
 * or the batch it makes with how far each has come, stopping it before it commits, the places a new
 * environment can go to, and the side threads of a thread: one made from a message, the ones that are open.
 *
 * A plan is good for a quarter of an hour and then must be made again; carrying it out names it. A request that
 * creates carries the key of the act, so that a second attempt after a lost answer makes nothing twice.
 */
import { queryOptions } from '@tanstack/react-query';

import { api, ApiError, ok, repeatable } from '@/api/client';
import { liveValue } from '@/api/liveValue';
import type {
  ForkBatch,
  ForkBatchPlan,
  ForkBatchPreflightBody,
  ForkCreateBody,
  ForkNowReply,
  ForkPlan,
  ForkRequest,
  Job,
  PlanBlocker,
  Place,
  SideState,
  Thread,
  ThreadSideCreateBody,
  ThreadSideCreateReply,
} from '@/generated/api-types';

export const forkKeys = {
  job: (id: string) => ['job', id] as const,
  batch: (id: string) => ['fork-batch', id] as const,
  places: (nodeId: string, template: string | null) => ['places', nodeId, template] as const,
  sides: (threadId: string, state: SideState) => ['thread-sides', threadId, state] as const,
};

/** Whether a job has ended, one way or another: nothing about it changes after that. */
export function isOver(state: Job['state']): boolean {
  return state === 'succeeded' || state === 'failed' || state === 'cancelled' || state === 'lost';
}

// ---------------------------------------------------------------- the plan

/** What a fork or a switch of one thread would do, before anything does. */
export function planFork(threadId: string, request: ForkRequest, signal?: AbortSignal): Promise<ForkPlan> {
  return ok(
    api.POST('/threads/{id}/forks/preflight', {
      params: { path: { id: threadId } },
      body: request,
      ...(signal === undefined ? {} : { signal }),
    }),
  );
}

/** Forks a thread at once with nothing to choose: the same topic, environment, provider, profile and model, with all of the history. */
export function forkNow(threadId: string, key: string, toTurn?: number): Promise<ForkNowReply> {
  return ok(
    api.POST('/threads/{id}/forks/now', { params: { path: { id: threadId } }, body: toTurn === undefined ? {} : { to_turn: toTurn }, ...repeatable(key) }),
  );
}

/** Carries a plan out: the job it makes, with the thread locked from this moment where it is a switch. */
export function startFork(threadId: string, body: ForkCreateBody, key: string): Promise<Job> {
  return ok(api.POST('/threads/{id}/forks', { params: { path: { id: threadId } }, body, ...repeatable(key) }));
}

/** What switching several threads at once would do: one plan for each, each with its own blockers. */
export function planBatch(body: ForkBatchPreflightBody, signal?: AbortSignal): Promise<ForkBatchPlan> {
  return ok(api.POST('/forks/preflight', { body, ...(signal === undefined ? {} : { signal }) }));
}

/** Carries a batch plan out: one job for each plan without blockers, all their threads locked at once. */
export function startBatch(body: ForkCreateBody, key: string): Promise<ForkBatch> {
  return ok(api.POST('/forks', { body, ...repeatable(key) }));
}

// ---------------------------------------------------------------- jobs and batches

/** A job's initial state; progress and its result arrive on its live feed. */
export const jobQuery = (id: string) =>
  queryOptions({
    queryKey: forkKeys.job(id),
    queryFn: ({ signal }): Promise<Job> => liveValue(forkKeys.job(id), signal),
  });

/** Stops a job before it commits; everything it made is taken away again. */
export function cancelJob(id: string): Promise<Job> {
  return ok(api.POST('/jobs/{id}/cancel', { params: { path: { id } } }));
}

/** A batch with how far each of its jobs has come, limited to the threads the caller may view; the others are counted. */
export const batchQuery = (id: string) =>
  queryOptions({
    queryKey: forkKeys.batch(id),
    queryFn: ({ signal }): Promise<ForkBatch> => liveValue(forkKeys.batch(id), signal),
  });

/** Stops the jobs of a batch that have not committed. */
export function cancelBatch(id: string): Promise<ForkBatch> {
  return ok(api.POST('/forks/{batch_id}/cancel', { params: { path: { batch_id: id } } }));
}

// ---------------------------------------------------------------- places

/** The hosts the caller may use for a new environment at a place, and the environments they manage that could take the thread instead. */
export const placesQuery = (nodeId: string, template: string | null = null) =>
  queryOptions({
    queryKey: forkKeys.places(nodeId, template),
    queryFn: ({ signal }): Promise<Place[]> =>
      ok(api.GET('/nodes/{id}/places', { params: { path: { id: nodeId }, query: template === null ? {} : { template_id: template } }, signal })),
    staleTime: 30_000,
  });

// ---------------------------------------------------------------- side threads

/** Starts a side thread of a thread from one of its turns, with the first message queued in it. */
export function startSide(threadId: string, body: ThreadSideCreateBody): Promise<ThreadSideCreateReply> {
  return ok(api.POST('/threads/{id}/side', { params: { path: { id: threadId } }, body }));
}

/** The side threads of a thread, oldest first: those that are open, or those that were closed. */
export const sidesQuery = (threadId: string, state: SideState = 'open') =>
  queryOptions({
    queryKey: forkKeys.sides(threadId, state),
    queryFn: ({ signal }): Promise<Thread[]> => ok(api.GET('/threads/{id}/side', { params: { path: { id: threadId }, query: { state } }, signal })),
  });

// ---------------------------------------------------------------- what an error says besides its code

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;

/** Whether a refusal says that the plan is no longer there to be carried out, because it is older than fifteen minutes or was used already: it is made again. */
export function planGone(error: unknown): boolean {
  return error instanceof ApiError && (error.is('preflight.expired') || error.status === 404);
}

/** The blockers that came with a refused plan (`plan.blocked`), as the plan would have carried them. */
export function blockersOf(error: unknown): PlanBlocker[] {
  if (!(error instanceof ApiError)) return [];
  const blockers = (error.problem as Record<string, unknown>).blockers;
  return Array.isArray(blockers) ? blockers.filter(isRecord).map((blocker) => blocker as unknown as PlanBlocker) : [];
}

/** The job that already holds a thread, when a plan or a confirmation was refused for that (`conflict.exists`). */
export function heldBy(error: unknown): string | null {
  if (!(error instanceof ApiError) || !error.is('conflict.exists')) return null;
  const job = (error.problem as Record<string, unknown>).job_id;
  return typeof job === 'string' ? job : null;
}

/** How many side threads are open and how many may be, when starting one was refused for the limit. */
export function sideLimitOf(error: unknown): { open: number; max: number } | null {
  if (!(error instanceof ApiError) || !error.is('thread.side_limit')) return null;
  const { open, max } = error.problem as Record<string, unknown>;
  return typeof open === 'number' && typeof max === 'number' ? { open, max } : null;
}
