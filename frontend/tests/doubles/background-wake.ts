/** Masked task/run facts follow the shared generated background-wake contract. */
import type { BackgroundTask, BackgroundWakeStatus, BackgroundWork, ThreadBackground } from '@/generated/api-types';

export function backgroundWork(more: Partial<BackgroundWork> = {}): BackgroundWork {
  return { id: 'recorded-build', item_id: 'build-item', turn_id: 'origin-turn', input_id: 'origin-input', author: null,
    kind: 'command', label: 'Build web', state: 'running', can_end: true, end_id: null, lost_at: null, ...more };
}

export function backgroundWake(more: Partial<BackgroundWakeStatus> = {}): BackgroundWakeStatus {
  return { setting: true, effective: true, refresh_deferred: false, tasks: [backgroundWork()], held_completions: 0,
    waiting_author: null, waiting_running: 0, ...more };
}

export function wakeCause(more: Partial<BackgroundTask> = {}): BackgroundTask {
  return { id: 'native-build', run_id: null, native_item_ref: 'build-ref', origin_turn: 'origin-turn', kind: 'command', label: 'Build web',
    state: 'completed', exit_code: 0, can_end: false, name: null, nickname: null, role: null, ...more };
}

export function backgroundRoster(more: Partial<ThreadBackground> = {}): ThreadBackground {
  return { supported: true, state: 'live', total: 0, items: [], as_of: null, next_cursor: null, list_ref: null, expires_at: null,
    can_end: true, reason: null, wake: backgroundWake(), ...more };
}
