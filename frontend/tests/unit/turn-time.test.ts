/** Work clocks freeze during a human wait and keep that wait out after resume, completion and reload. */
import { describe, expect, it } from 'vitest';
import { turnActiveMs, turnWaiting } from '@/features/timeline/turnTime';

const START = Date.parse('2026-10-15T10:00:00Z');
const at = (ms: number) => new Date(START + ms).toISOString();
const working = { started_at: at(0), completed_at: null, waiting_since: null, waited_ms: 0 };

describe('the time a turn spent working', () => {
  it('holds at seven working seconds through a ten-minute wait, resumes and finishes with twelve', () => {
    const waiting = { ...working, waiting_since: at(7_000) };
    expect(turnWaiting(waiting)).toBe(true);
    expect(turnActiveMs(waiting, START + 607_000)).toBe(7_000);
    expect(turnActiveMs(waiting, START + 667_000)).toBe(7_000);
    const resumed = { ...working, waited_ms: 600_000 };
    expect(turnWaiting(resumed)).toBe(false);
    expect(turnActiveMs(resumed, START + 612_000)).toBe(12_000);
    const completed = { ...resumed, completed_at: at(612_000) };
    expect(turnActiveMs(completed, START + 86_400_000)).toBe(12_000);
  });

  it('uses the completed human-wait total once even when several requests overlapped', () => {
    const waiting = { ...working, waited_ms: 120_000, waiting_since: at(132_000) };
    expect(turnActiveMs(waiting, START + 732_000)).toBe(12_000);
    expect(turnActiveMs({ ...working, waited_ms: 720_000, completed_at: at(739_000) }, START + 900_000)).toBe(19_000);
  });

  it('keeps old records readable with zero known wait and never displays a negative working span', () => {
    expect(turnActiveMs({ started_at: at(0), completed_at: at(12_000) }, START + 1_000_000)).toBe(12_000);
    expect(turnWaiting({})).toBe(false);
    expect(turnActiveMs({ ...working, waited_ms: 60_000 }, START + 1_000)).toBe(0);
    expect(turnActiveMs({ ...working, started_at: null }, START)).toBeNull();
  });
});
