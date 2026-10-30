/** The few things an export of the audit trail is made of: the days between two days, the day it is today, the most days one file may span, and what the file is called.
 *
 * Days are written as the address and the date field write them, `2026-10-01`, and counted in UTC,
 * which is the clock the trail is kept by.
 */

/** The most days one file of the trail may hold. */
export const EXPORT_MOST_DAYS = 366;

const DAY_MS = 86_400_000;

/** How many days after `from` the day `to` is: negative where it is before. `NaN` for a day that is not one. */
export function daysBetween(from: string, to: string): number {
  return Math.round((Date.parse(`${to}T00:00:00Z`) - Date.parse(`${from}T00:00:00Z`)) / DAY_MS);
}

/** The day, `offset` days from today, in UTC. */
export function today(offset: number, now: number = Date.now()): string {
  return new Date(now + offset * DAY_MS).toISOString().slice(0, 10);
}

/** What the file of a span is called. */
export function exportName(from: string, to: string, format: 'csv' | 'jsonl'): string {
  return `orch-audit-${from}-to-${to}.${format}`;
}
