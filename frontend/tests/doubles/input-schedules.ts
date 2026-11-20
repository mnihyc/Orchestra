/** Synthetic ordinary schedule projections; agent origin is separate from the acting person's identity. */
import type { InputSchedule } from '@/api/inputSchedules';
import type { InputScheduleRun } from '@/generated/api-types';
import { ADA } from './chat-rows';
export const SCHEDULE = '01950000-0000-7000-8000-000000000011';
export const SCHEDULE_THREAD = 'scheduled-thread';
export const SCHEDULE_TOPIC = 'schedule-topic';
export function scheduleRun(more: Partial<InputScheduleRun> = {}): InputScheduleRun {
  return { id: 'native-run', occurrence: '2030-01-01T21:30:00Z', scheduled_for: '2030-01-01T21:30:00Z', created_at: '2030-01-01T21:30:00Z', started_at: '2030-01-01T21:30:01Z',
    state: 'started', input_id: 'scheduled-input', turn_id: 'scheduled-turn', thread: { id: SCHEDULE_THREAD, kind: 'thread', name: 'Review', redacted: false }, reason: null, ...more };
}
export function inputSchedule(more: Partial<InputSchedule> = {}): InputSchedule {
  return { id: SCHEDULE, kind: 'agent', agent: { thread_id: SCHEDULE_THREAD, agent_id: 'research-agent', source_turn_id: 'source-turn', until: '2030-01-02T21:30:00Z', max_runs: 20, runs: 0 },
    topic: { id: SCHEDULE_TOPIC, kind: 'topic', name: 'Release', redacted: false }, author: ADA, name: 'Check the CI run', plan: false, delivery: 'queue',
    destination: { thread: { id: SCHEDULE_THREAD, kind: 'thread', name: 'Review', redacted: false }, new_thread: false }, definition: null,
    timing: { kind: 'once', at: '2030-01-01T21:30:00Z', zone: 'UTC' }, state: 'active', next_run_at: '2030-01-01T21:30:00Z', revision: 1,
    last_run: null, created_at: '2030-01-01T20:30:00Z', updated_at: '2030-01-01T20:30:00Z', topic_seq: 2, ...more };
}
