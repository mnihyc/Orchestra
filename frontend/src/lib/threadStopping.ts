/** Accepted Stop state has its own clock and version, shared by rows, acknowledgements and live events. */
import type { Thread, ThreadStopping, Turn } from '@/generated/api-types';

export function mergeStopping(before: ThreadStopping | null | undefined, next: ThreadStopping | null | undefined): ThreadStopping | null {
  if (next === null || next === undefined) return before ?? null;
  if (before === null || before === undefined) return next;
  return next.version > before.version || next.version === before.version && next.thread_seq > before.thread_seq ? next : before;
}

export function stoppingSince(thread: Thread | null, turn: Turn | null = null, observedSeq = 0): string | null {
  const state = thread?.stopping;
  if (state !== null && state !== undefined && (state.pending || Math.max(observedSeq, thread?.seq ?? 0) < state.thread_seq)) return state.requested_at;
  return thread !== null && turn !== null && turn.id === thread.current_turn_id && thread.state !== 'idle'
    && (turn.state === 'starting' || turn.state === 'running') && turn.interrupted_by !== null
    && (state === null || state === undefined || turn.seq_updated > state.thread_seq) ? turn.interrupted_at ?? null : null;
}

/** Event data is the same small observation as the HTTP answer, without accepting arbitrary row patches. */
export function readStopping(data: unknown): ThreadStopping | null {
  if (typeof data !== 'object' || data === null || !('version' in data) || !('thread_seq' in data)
    || !('requested_at' in data) || !('pending' in data)
    || typeof data.version !== 'number' || !Number.isSafeInteger(data.version) || data.version < 1
    || typeof data.thread_seq !== 'number' || !Number.isSafeInteger(data.thread_seq) || data.thread_seq < 0
    || typeof data.requested_at !== 'string' || !Number.isFinite(Date.parse(data.requested_at)) || typeof data.pending !== 'boolean') return null;
  return { version: data.version, thread_seq: data.thread_seq, requested_at: data.requested_at, pending: data.pending };
}
