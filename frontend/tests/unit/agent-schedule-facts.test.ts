/** Forged hints, foreign origins and a merely queued run never label an ordinary message as an agent wake. */
import { expect, it } from 'vitest';
import { belongsToThread, hintedSchedule, isAgentSchedule, runWokeInput } from '@/features/schedules/facts';
import { inputSchedule, scheduleRun, SCHEDULE, SCHEDULE_THREAD } from '../doubles/input-schedules';

it('uses a native client id only to choose the authorized lookup', () => {
  expect(hintedSchedule(`agent-schedule:${SCHEDULE}:2030-01-01T21:30:00Z`)).toBe(SCHEDULE);
  for (const value of [null, undefined, 'Check CI', `agent-schedule:${SCHEDULE}`, 'agent-schedule:another/path:time']) expect(hintedSchedule(value)).toBeNull();
});
it('keeps people and agent rows on their actual thread without using the note as origin', () => {
  const ordinary = inputSchedule({ kind: undefined, agent: undefined });
  expect(belongsToThread(ordinary, SCHEDULE_THREAD)).toBe(true); expect(isAgentSchedule(ordinary, SCHEDULE_THREAD)).toBe(false);
  expect(isAgentSchedule(inputSchedule(), SCHEDULE_THREAD)).toBe(true);
  expect(isAgentSchedule(inputSchedule({ agent: { ...inputSchedule().agent!, thread_id: 'foreign' } }), SCHEDULE_THREAD)).toBe(false);
  expect(belongsToThread(inputSchedule({ destination: { thread: null, new_thread: true } }), SCHEDULE_THREAD)).toBe(false);
});
it('requires the actual started run, input, turn and own destination together', () => {
  const schedule = inputSchedule(), run = scheduleRun();
  expect(runWokeInput(schedule, run, SCHEDULE_THREAD, 'scheduled-input', 'scheduled-turn')).toBe(true);
  expect(runWokeInput(schedule, run, SCHEDULE_THREAD, 'forged-input', 'scheduled-turn')).toBe(false);
  expect(runWokeInput(schedule, run, SCHEDULE_THREAD, 'scheduled-input', 'other-turn')).toBe(false);
  expect(runWokeInput(schedule, run, SCHEDULE_THREAD, 'scheduled-input', null)).toBe(false);
  for (const state of ['queued', 'blocked', 'failed', 'skipped'] as const) expect(runWokeInput(schedule, { ...run, state }, SCHEDULE_THREAD, 'scheduled-input', 'scheduled-turn')).toBe(false);
  expect(runWokeInput(schedule, { ...run, thread: { ...run.thread!, id: 'foreign' } }, SCHEDULE_THREAD, 'scheduled-input', 'scheduled-turn')).toBe(false);
  expect(runWokeInput({ ...schedule, kind: undefined, agent: undefined }, run, SCHEDULE_THREAD, 'scheduled-input', 'scheduled-turn')).toBe(false);
});
