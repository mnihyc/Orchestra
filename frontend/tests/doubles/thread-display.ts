/** Explicit server icon fixtures keep execution state and reader presentation separate. */
import type { ThreadDisplay, ThreadDisplayRecovery, ThreadDisplayState, ThreadState } from '../../src/generated/api-types.ts';

export function display(state: ThreadDisplayState, more: Partial<ThreadDisplay> = {}): ThreadDisplay {
  return { state, seq: 1, read_seq: 0, queued_inputs: 0, held_inputs: 0, waiting_inputs: 0, waiting_for_answer: false, blocked_reason: null, finished_at: null, ...more };
}

export function executionDisplay(state: ThreadState, more: Partial<ThreadDisplay> = {}): ThreadDisplay {
  return display(state === 'starting' ? 'running' : state, more);
}

export function recoveryDisplay(more: Partial<ThreadDisplayRecovery> = {}): ThreadDisplayRecovery {
  return { rule_id: 'busy', step: 'retry', attempt: 2, attempts: 3, due_at: '2026-10-28T00:00:55.000Z',
    continuing: false, failure_kind: 'provider_busy', stop_reason: null, count: null, window_secs: null, ...more };
}
