/** Orders server-computed thread icons across the existing work and personal feeds. */
import type { ThreadDisplay, ThreadDisplayState, TreeNode } from '@/generated/api-types';

/** Contained work uses the same priority on every place; idle and archived add no mark. */
export const THREAD_URGENCY = ['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished'] as const satisfies readonly ThreadDisplayState[];

export type ThreadUrgencyState = (typeof THREAD_URGENCY)[number];

/** Quiet and archived states contribute no work mark to a place. */
export function isThreadUrgency(state: ThreadDisplayState | null | undefined): state is ThreadUrgencyState {
  return THREAD_URGENCY.some((candidate) => candidate === state);
}

const STATES: readonly ThreadDisplayState[] = ['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished', 'idle', 'archived'];

function isRecoveryDisplay(value: unknown): boolean {
  if (value === undefined || value === null) return true;
  if (typeof value !== 'object') return false;
  const row = value as Record<string, unknown>;
  const nonnegative = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
  return typeof row.rule_id === 'string' && typeof row.continuing === 'boolean'
    && nonnegative(row.attempt)
    && ['attempts', 'count', 'window_secs'].every((key) => row[key] === null || nonnegative(row[key]))
    && (row.due_at === null || typeof row.due_at === 'string' && Number.isFinite(Date.parse(row.due_at)))
    && (row.step === null || typeof row.step === 'string' && ['retry', 'continue', 'failover', 'wait'].includes(row.step))
    && typeof row.failure_kind === 'string' && ['provider_busy', 'provider_unreachable', 'model_refused', 'model_unavailable', 'access', 'provider_exhausted', 'provider_unauthorized', 'context_overflow', 'harness_error', 'any'].includes(row.failure_kind)
    && (row.stop_reason === null || typeof row.stop_reason === 'string' && ['repeat_limit', 'history_changed', 'author_missing', 'author_lacks_interact', 'author_lacks_use', 'switch_failed', 'switch_blocked', 'stopped_by_person', 'candidate_not_allowed', 'embedding_identity', 'origin_missing', 'rule_unavailable', 'rules_changed', 'auto_disabled', 'repeat_limit_changed', 'service_precondition', 'delay_unrepresentable', 'candidate_parameters', 'candidate_unavailable', 'origin_authority'].includes(row.stop_reason));
}

export function isThreadDisplay(value: unknown): value is ThreadDisplay {
  if (typeof value !== 'object' || value === null) return false;
  const row = value as Record<string, unknown>;
  return STATES.some((state) => state === row.state)
    && ['seq', 'read_seq', 'queued_inputs', 'held_inputs', 'waiting_inputs'].every((field) => typeof row[field] === 'number' && Number.isSafeInteger(row[field]) && row[field] >= 0)
    && typeof row.waiting_for_answer === 'boolean'
    && (row.blocked_reason === null || typeof row.blocked_reason === 'string')
    && (row.finished_at === null || typeof row.finished_at === 'string')
    && isRecoveryDisplay(row.recovery);
}

/** A delayed work frame cannot put back a finished check that the personal feed already cleared. */
export function currentThreadDisplay(incoming: ThreadDisplay | undefined, held: ThreadDisplay | undefined): ThreadDisplay | undefined {
  if (incoming === undefined) return held;
  if (held === undefined || incoming.seq > held.seq || incoming.seq === held.seq && incoming.read_seq >= held.read_seq) return incoming;
  return held;
}

const contained = new WeakMap<ReadonlyMap<string, TreeNode>, ReadonlyMap<string, ThreadUrgencyState>>();

/** Held browser rows may predate the server's display metadata. */
function cachedDisplay(thread: { display?: ThreadDisplay | null } | null | undefined): ThreadDisplay | null | undefined {
  return thread?.display;
}

/** Fold authorized thread presentations up their actual parent chain once per held tree. */
export function containedThreadStates(rows: ReadonlyMap<string, TreeNode>): ReadonlyMap<string, ThreadUrgencyState> {
  const held = contained.get(rows);
  if (held !== undefined) return held;
  const states = new Map<string, ThreadUrgencyState>();
  for (const row of rows.values()) {
    const state = cachedDisplay(row.thread)?.state;
    if (row.deleted || row.archived || !isThreadUrgency(state)) continue;
    const rank = THREAD_URGENCY.indexOf(state);
    for (let id = row.parent_id; id !== null; id = rows.get(id)?.parent_id ?? null) {
      const before = states.get(id);
      // An equal or stronger mark has already reached this place's ancestors.
      // This also ends a corrupt cycle without allocating one visited set per thread.
      if (before !== undefined && THREAD_URGENCY.indexOf(before) <= rank) break;
      states.set(id, state);
    }
  }
  contained.set(rows, states);
  return states;
}
