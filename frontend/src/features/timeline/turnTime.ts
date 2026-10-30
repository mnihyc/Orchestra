/** A turn's working time excludes the human waits recorded by its live feed. */
import type { Turn } from '@/generated/api-types';

// Old retained records have no wait counters. Their missing fields mean zero known waiting time.
type Timing = Pick<Turn, 'started_at' | 'completed_at'> & Partial<Pick<Turn, 'waiting_since' | 'waited_ms'>>;

/** The current human wait freezes the clock; completed waits are deducted once by the server. */
export function turnActiveMs(turn: Timing, now: number): number | null {
  if (turn.started_at === null) return null;
  const end = turn.completed_at ?? turn.waiting_since;
  return Math.max(0, (end === null || end === undefined ? now : Date.parse(end)) - Date.parse(turn.started_at) - (turn.waited_ms ?? 0));
}

/** Whether the server has an open human-wait interval for this turn. */
export function turnWaiting(turn: Pick<Timing, 'waiting_since'>): boolean {
  return Boolean(turn.waiting_since);
}
