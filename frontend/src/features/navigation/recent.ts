/** The shared recent order, local-calendar groups and Navigation's listing predicates. */
import type { EverythingFilter } from '@/api/everything';
import type { EverythingKind } from '@/generated/api-types';
import { ApiError } from '@/api/client';
import { calendarDay } from '@/lib/time';

export type NavigationArrangement = 'tree' | 'recent';
export type RecentOrder = 'opened' | 'active';
export const RECENT_GROUPS = ['today', 'yesterday', 'week', 'earlier'] as const;
export type RecentGroup = typeof RECENT_GROUPS[number];
/** The preceding week includes days two through seven; today and yesterday have their own headings. */
const WEEK_DAYS = 7;
/** Authoritative authentication, access and deletion responses retire cached identities. */
const RETIRES_LIST = new Set([401, 403, 404, 410]);

export function listingLostAccess(error: unknown): boolean {
  return error instanceof ApiError && RETIRES_LIST.has(error.status);
}

/** The local calendar date is the only clock input that can change a date group. */
export function recentDay(at: number, timeZone?: string): number {
  return calendarDay(at, { locale: 'en', timeZone });
}

export function recentGroup(at: string | null | undefined, now: number, timeZone?: string): RecentGroup {
  return recentGroupOnDay(at, recentDay(now, timeZone), timeZone);
}

export function recentGroupOnDay(at: string | null | undefined, today: number, timeZone?: string): RecentGroup {
  const value = at === null || at === undefined ? NaN : Date.parse(at);
  if (!Number.isFinite(value)) return 'earlier';
  const days = today - recentDay(value, timeZone);
  return days <= 0 ? 'today' : days === 1 ? 'yesterday' : days <= WEEK_DAYS ? 'week' : 'earlier';
}

export function recentAt(row: { last_opened?: string | null; last_activity: string | null }, order: RecentOrder): string | null {
  return order === 'opened' ? row.last_opened ?? null : row.last_activity;
}

/** Date groups remain in calendar order while each group retains the server's chosen column order. */
export function groupedRecents<T extends { last_opened?: string | null; last_activity: string | null }>(rows: readonly T[], order: RecentOrder, now: number, timeZone?: string): T[] {
  return groupedRecentsOnDay(rows, order, recentDay(now, timeZone), timeZone);
}

export function groupedRecentsOnDay<T extends { last_opened?: string | null; last_activity: string | null }>(rows: readonly T[], order: RecentOrder, today: number, timeZone?: string): T[] {
  const groups = new Map<RecentGroup, T[]>(RECENT_GROUPS.map((group) => [group, []]));
  for (const row of rows) groups.get(recentGroupOnDay(recentAt(row, order), today, timeZone))?.push(row);
  return RECENT_GROUPS.flatMap((group) => groups.get(group) ?? []);
}

/** The server applies these choices before the limit, including descendants hidden with Threads. */
export function navigationFilter(order: RecentOrder, q: string, show: { threads: boolean; documents: boolean; chats: boolean }, showArchived: boolean, hideDone: boolean, participating: boolean, keep?: string): EverythingFilter {
  const kinds: EverythingKind[] = ['org', 'project', 'topic'];
  if (show.threads) kinds.push('thread');
  if (show.documents) kinds.push('document');
  if (show.chats) kinds.push('chat');
  return { scope: 'mine', kinds, q, owner: null, createdFrom: '', createdTo: '', sort: order === 'opened' ? 'opened' : 'last_activity', direction: 'desc',
    recent: order, navigation: true, showArchived, hideDone, participating, ...(keep === undefined ? {} : { keep }) };
}
