/** Managed future inputs use the existing topic snapshot, schedule history and cancellation routes. */
import { infiniteQueryOptions, queryOptions, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { ApiError, accepted, api, done, ok, repeatable } from '@/api/client';
import { feedSequences, record } from '@/api/cache';
import { supplyOverviewPart, type NativeOverviewPart } from '@/api/overviewParts';
import { scheduledReads, suppliedRead } from '@/api/readScheduler';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { InputSchedule as NativeSchedule, InputScheduleCreateBody, InputScheduleUpdateBody, InputScheduleWriteResponse, InputScheduleRunPage, InputTiming as NativeTiming, Problem } from '@/generated/api-types';

/** Additive native fields until central contract generation; people's existing rows keep their shape. */
export type InputTiming = NativeTiming | { kind: 'cron'; expression: string; zone: string };
export interface AgentInputSchedule { thread_id: string; agent_id: string; source_turn_id: string; until: string; max_runs: number; runs: number }
export type InputSchedule = Omit<NativeSchedule, 'timing'> & { timing: InputTiming; kind?: 'agent' | null; agent?: AgentInputSchedule | null };
/** Public fields of one current-thread continuation, separate from the full schedule definition/list. */
export type PendingAgentSchedule = Pick<InputSchedule, 'id' | 'name' | 'state'> & { next_run_at: string };
export interface ThreadPendingAgentSchedules { seq: number; items: PendingAgentSchedule[] }
export const scheduleKeys = {
  topic: (id: string) => ['topic-schedules', id] as const,
  one: (id: string) => ['input-schedule', id] as const,
  runs: (id: string) => ['input-schedule-runs', id] as const,
};
/** Match the ordinary native page bound; continue through opaque cursors, never a run-count cap. */
const RUN_PAGE = 100;
function current(reader: ReturnType<typeof libraryReader>, signal: AbortSignal): void {
  signal.throwIfAborted();
  if (!sameLibraryReader(reader)) throw new DOMException('The schedule reader changed', 'AbortError');
}
export const topicSchedulesQuery = (topic: string) => {
  const reader = libraryReader();
  return queryOptions({ queryKey: [...scheduleKeys.topic(topic), reader] as const,
    queryFn: async ({ signal }): Promise<InputSchedule[]> => {
      current(reader, signal);
      const state = await ok(api.GET('/topics/{id}/sync', { params: { path: { id: topic } }, signal }));
      current(reader, signal); return state.input_schedules;
    }, retry: false });
};

const PENDING = 'thread-pending-agent-schedules';
const pendingKey = (thread: string, topic: string, reader: ReturnType<typeof libraryReader>) => [PENDING, thread, topic, reader] as const;
const publicLine = (value: unknown): value is PendingAgentSchedule => record(value)
  && typeof value.id === 'string' && typeof value.name === 'string' && value.state === 'active'
  && typeof value.next_run_at === 'string' && Number.isFinite(Date.parse(value.next_run_at));
const projection = (value: unknown): value is ThreadPendingAgentSchedules => record(value)
  && typeof value.seq === 'number' && Number.isSafeInteger(value.seq) && value.seq >= 0
  && Array.isArray(value.items) && value.items.every(publicLine);
const problem = (value: unknown): value is Problem => record(value)
  && typeof value.code === 'string' && typeof value.status === 'number'
  && typeof value.type === 'string' && typeof value.title === 'string'
  && typeof value.detail === 'string' && typeof value.request_id === 'string';

function pendingPart(value: unknown): NativeOverviewPart<ThreadPendingAgentSchedules> | undefined {
  if (value === undefined || value === null) return undefined;
  if (record(value) && value.status === 'ready' && projection(value.data)) return { status: 'ready', data: value.data };
  if (record(value) && value.status === 'failed' && problem(value.problem)
    && (value.retry_after === undefined || value.retry_after === null || typeof value.retry_after === 'number')) {
    return { status: 'failed', problem: value.problem, retry_after: value.retry_after ?? null };
  }
  if (record(value) && value.status === 'omitted') return { status: 'omitted' };
  throw new Error('The pending schedule projection is malformed');
}
function remember(topic: string, seq: number): void {
  const feed = `topic:${topic}`;
  feedSequences.set(feed, Math.max(feedSequences.get(feed) ?? 0, seq));
}

/** Install this exact thread subset; it must never populate the complete topic-list key. */
export function seedPendingAgentSchedules(cache: QueryClient, thread: string, topic: string, value: unknown): void {
  const key = pendingKey(thread, topic, libraryReader());
  let part: NativeOverviewPart<ThreadPendingAgentSchedules> | undefined;
  try { part = pendingPart(value); }
  catch (error) {
    const query = cache.getQueryCache().build(cache, { queryKey: key });
    if (query.state.fetchStatus === 'fetching') void query.cancel({ revert: false, silent: true });
    query.setState({ data: undefined, status: 'error', fetchStatus: 'idle',
      error: error instanceof Error ? error : new Error('The pending schedule projection is malformed') });
    scheduledReads.supply(key, [key]); return;
  }
  if (part === undefined || part.status === 'omitted') return;
  const held = cache.getQueryData<ThreadPendingAgentSchedules>(key);
  if (part.status === 'ready' && held !== undefined && held.seq > part.data.seq) return;
  const query = cache.getQueryCache().find({ queryKey: key, exact: true });
  if (query?.state.fetchStatus === 'fetching') void query.cancel({ revert: false, silent: true });
  supplyOverviewPart(cache, key, part);
  // A native supplier settles the flight it superseded, including an abort-ignoring transport.
  cache.getQueryCache().find({ queryKey: key, exact: true })?.setState({ fetchStatus: 'idle' });
  scheduledReads.supply(key, [key]);
  if (part.status === 'ready') remember(topic, part.data.seq);
}

/** Retry/live repair uses the same declared opening operation, without replacing its timeline window. */
export const pendingAgentSchedulesQuery = (thread: string, topic: string) => {
  const reader = libraryReader(), key = pendingKey(thread, topic, reader);
  return queryOptions({ queryKey: key, ...suppliedRead(key), retry: false,
    queryFn: async ({ signal }): Promise<ThreadPendingAgentSchedules> => {
      current(reader, signal);
      // Only the pending projection is consumed; one item is the smallest native opening window.
      const opening = await ok(api.GET('/threads/{id}/opening', { params: { path: { id: thread }, query: { limit: 1 } }, signal }));
      current(reader, signal);
      if (opening.sync.thread.id !== thread || opening.sync.thread.topic_id !== topic) throw new Error('The schedule opening belongs to another thread');
      const part = pendingPart('pending_agent_schedules' in opening ? opening.pending_agent_schedules : undefined);
      scheduledReads.supply(key, [key]);
      if (part?.status === 'failed') throw new ApiError(part.problem, (part.retry_after === null || part.retry_after === undefined) ? null : new Date(Date.now() + part.retry_after * 1000));
      if (part?.status !== 'ready') throw new Error('The current opening did not provide pending schedules');
      remember(topic, part.data.seq); return part.data;
    } });
};

export function pendingScheduleKey(key: QueryKey): key is ReturnType<typeof pendingKey> {
  return key[0] === PENDING && typeof key[1] === 'string' && typeof key[2] === 'string' && Array.isArray(key[3]);
}

/** A freshly refused optional projection keeps its failure until explicit Retry. */
export function pendingSchedulesRefused(cache: QueryClient, thread: string, topic: string): boolean {
  const key = pendingKey(thread, topic, libraryReader());
  return scheduledReads.owns(key) && cache.getQueryState(key)?.status === 'error';
}
export const inputScheduleQuery = (id: string) => {
  const reader = libraryReader();
  return queryOptions({ queryKey: [...scheduleKeys.one(id), reader] as const,
    queryFn: async ({ signal }): Promise<InputSchedule> => {
      current(reader, signal);
      const row = await ok(api.GET('/input-schedules/{id}', { params: { path: { id } }, signal }));
      current(reader, signal); return row;
    }, retry: false });
};
export const inputScheduleRunsQuery = (id: string) => {
  const reader = libraryReader();
  return infiniteQueryOptions({ queryKey: [...scheduleKeys.runs(id), reader] as const, initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<InputScheduleRunPage> => {
      current(reader, signal);
      const page = await ok(api.GET('/input-schedules/{id}/runs', { params: { path: { id }, query: { limit: RUN_PAGE, ...(pageParam === null ? {} : { cursor: pageParam }) } }, signal }));
      current(reader, signal); return page;
    }, getNextPageParam: (page) => page.next_cursor, retry: false });
};
export function cancelInputSchedule(id: string): Promise<void> {
  return done(api.DELETE('/input-schedules/{id}', { params: { path: { id } } }));
}

/** Browser-authored saves freeze the supplied draft; acknowledgement never requires readback. */
export function createInputSchedule(topic: string, body: InputScheduleCreateBody, key: string): Promise<InputScheduleWriteResponse> {
  return accepted(api.POST('/topics/{id}/input-schedules', { params: { path: { id: topic } }, body, ...repeatable(key) }));
}
export function updateInputSchedule(id: string, body: InputScheduleUpdateBody, key: string): Promise<InputScheduleWriteResponse> {
  return accepted(api.PATCH('/input-schedules/{id}', { params: { path: { id } }, body, ...repeatable(key) }));
}
