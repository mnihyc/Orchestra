/** Display joins use actual schedule/run identities, never note text or a chosen client string as proof. */
import type { InputSchedule } from '@/api/inputSchedules';
import type { InputScheduleRun } from '@/generated/api-types';

export function belongsToThread(schedule: InputSchedule, thread: string): boolean {
  const destination = schedule.destination.thread;
  return !schedule.destination.new_thread && destination?.kind === 'thread' && destination.id === thread
    && (schedule.kind !== 'agent' || schedule.agent?.thread_id === thread);
}
export function isAgentSchedule(schedule: InputSchedule, thread: string): boolean {
  return schedule.kind === 'agent' && (schedule.agent !== null && schedule.agent !== undefined) && belongsToThread(schedule, thread);
}
/** A hint only chooses which authorized metadata/history to ask for. */
export function hintedSchedule(clientId: string | null | undefined): string | null {
  return /^agent-schedule:([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}):.+$/iu.exec(clientId ?? '')?.[1] ?? null;
}
export function runWokeInput(schedule: InputSchedule, run: InputScheduleRun, thread: string, input: string, turn: string | null): boolean {
  return isAgentSchedule(schedule, thread) && turn !== null && run.state === 'started' && run.input_id === input
    && run.turn_id === turn && run.thread?.id === thread && run.thread.kind === 'thread';
}
