/** A reviewed native set stays fixed through paging and confirmation; a new command belongs to a new review. */
import { fetchBackground } from '@/api/background';
import type { BackgroundEndRequest, BackgroundEndResult, ThreadBackground } from '@/generated/api-types';

/** Reads every page of one snapshot before offering its complete End all count. */
export async function loadReview(thread: string, signal: AbortSignal): Promise<ThreadBackground> {
  const first = await fetchBackground(thread, undefined, signal);
  const captured = { ...first, items: [...first.items] };
  const seen = new Set<string>();
  let cursor = first.next_cursor;
  while (cursor !== null) {
    if (seen.has(cursor)) throw new Error('the native list repeated a page');
    seen.add(cursor);
    const page = await fetchBackground(thread, cursor, signal);
    if (page.list_ref !== first.list_ref || page.total !== first.total) throw new Error('the native review changed during transfer');
    captured.items.push(...page.items);
    captured.expires_at = page.expires_at;
    captured.can_end = page.can_end;
    cursor = page.next_cursor;
  }
  captured.next_cursor = null;
  return captured;
}

/** A partial, unavailable or untransferred set cannot authorize End all. */
export function canEndAll(roster: ThreadBackground): boolean {
  return roster.can_end && roster.state === 'live' && roster.list_ref !== null && roster.total !== null && roster.total > 0 && roster.next_cursor === null && roster.items.length === roster.total;
}

/** Captures the request at confirmation, keeping later native work out of the accepted set. */
export function reviewedRequest(roster: ThreadBackground, ids: readonly string[] | 'all'): BackgroundEndRequest | null {
  if (!roster.can_end || roster.list_ref === null || roster.state !== 'live') return null;
  if (ids === 'all') {
    const count = roster.total;
    return count !== null && canEndAll(roster) ? { list_ref: roster.list_ref, all: true, count } : null;
  }
  const selected = new Set(ids);
  if (selected.size === 0 || selected.size > 200 || selected.size !== ids.length || !ids.every((id) => roster.items.some((item) => item.id === id))) return null;
  return { list_ref: roster.list_ref, command_ids: [...ids] };
}

/** Reads only factual mutation outcomes; no job result can become the next live roster. */
export function endResults(value: unknown): BackgroundEndResult['results'] {
  if (typeof value !== 'object' || value === null || !('results' in value) || !Array.isArray(value.results)) return [];
  return (value.results as BackgroundEndResult['results']).filter((entry) => typeof entry.id === 'string' && ['ended', 'already_ended', 'still_running', 'unknown'].includes(entry.state));
}
