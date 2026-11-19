/** What the Inbox shows of the rows it holds: the three filters, the days the rows fall on, and the place a row is about.
 *
 * Pure functions of rows, so that the rules can be tested without a screen: which rows a filter
 * keeps, how they group by day in the person's time zone (a day is the person's day, not the
 * server's), and how a row names its place.
 */
import { MENTION_KINDS } from '@/features/inbox/kinds';
import type { Notification } from '@/generated/api-types';
import { type UserState, withinPages } from '@/realtime/stores/user';

export type InboxFilter = 'all' | 'unread' | 'mentions';

export const FILTERS: readonly InboxFilter[] = ['all', 'unread', 'mentions'];

/**
 * The rows a filter keeps, newest first. Unread is every unread row held, wherever it lies; the
 * others only the rows within the pages loaded from the newest, since beyond them rows are missing
 * that would stand between.
 */
export function filterRows(rows: readonly Notification[], filter: InboxFilter, pages: Pick<UserState, 'edge' | 'reachedEnd'>): Notification[] {
  switch (filter) {
    case 'unread':
      return rows.filter((row) => row.read_at === null);
    case 'mentions':
      return rows.filter((row) => MENTION_KINDS.includes(row.kind) && withinPages(pages, row));
    case 'all':
      return rows.filter((row) => withinPages(pages, row));
  }
}

/** The calendar day of a moment in a time zone, as `2026-10-12`. */
export function dayKey(moment: string | number | Date, timeZone: string | undefined): string {
  const date = moment instanceof Date ? moment : new Date(moment);
  try {
    return new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(date);
  } catch {
    // A zone name this browser does not know: its own zone is better than no day at all.
    return new Intl.DateTimeFormat('en-CA', { year: 'numeric', month: '2-digit', day: '2-digit' }).format(date);
  }
}

/** The day before a day, by the calendar and not by 24 hours, which the clock changes make shorter and longer. */
export function previousDay(key: string): string {
  const [year, month, day] = key.split('-').map(Number);
  const before = new Date(Date.UTC(year ?? 1970, (month ?? 1) - 1, (day ?? 1) - 1));
  return before.toISOString().slice(0, 10);
}

export interface DayGroup {
  key: string;
  rows: Notification[];
}

/** Rows (newest first) in runs that fall on one day. */
export function groupByDay(rows: readonly Notification[], timeZone: string | undefined): DayGroup[] {
  const groups: DayGroup[] = [];
  for (const row of rows) {
    const key = dayKey(row.created_at, timeZone);
    const group = groups[groups.length - 1];
    if (group?.key === key) group.rows.push(row);
    else groups.push({ key, rows: [row] });
  }
  return groups;
}

/** A day written for a heading: "Monday, 5 October" and with the year when it is not this one. */
export function writeDay(key: string, today: string, locale: string): string {
  const sameYear = key.slice(0, 4) === today.slice(0, 4);
  // Noon in UTC is the same calendar day in every zone the formatter is asked in below (UTC).
  return new Intl.DateTimeFormat(locale, {
    timeZone: 'UTC',
    weekday: 'long',
    month: 'long',
    day: 'numeric',
    year: sameYear ? undefined : 'numeric',
  }).format(new Date(`${key}T12:00:00Z`));
}

/** The place a row is about, as the row names it: a node's id for a thread, a project or a topic. */
export function placeOf(row: Pick<Notification, 'node' | 'thread_id'>): string | null {
  return row.node?.id ?? row.thread_id;
}
