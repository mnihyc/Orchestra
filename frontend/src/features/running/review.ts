/** A reviewed native set stays fixed through paging and confirmation; a new command belongs to a new review. */
import { fetchBackground } from '@/api/background';
import type { BackgroundCommand, BackgroundEndReason, BackgroundEndRequest, BackgroundEndResult, BackgroundWork, ThreadBackground } from '@/generated/api-types';

export type BackgroundEndTarget = { task: Pick<BackgroundWork, 'id' | 'item_id' | 'turn_id'> } | { command: Pick<BackgroundCommand, 'id' | 'segment_id' | 'item_id' | 'turn_id'> };

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
    if (page.wake !== undefined && page.wake !== null) {
      const tasks = new Map((captured.wake?.tasks ?? []).map((task) => [task.id, task]));
      for (const task of page.wake.tasks) {
        const previous = tasks.get(task.id);
        if (previous !== undefined && (previous.item_id !== task.item_id || previous.turn_id !== task.turn_id || previous.end_id !== null && task.end_id !== null && previous.end_id !== task.end_id)) throw new Error('the native task changed during transfer');
        tasks.set(task.id, { ...task, end_id: task.end_id ?? previous?.end_id ?? null });
      }
      captured.wake = { ...page.wake, tasks: [...tasks.values()] };
    }
    captured.expires_at = page.expires_at;
    captured.can_end = page.can_end;
    captured.end_reason = page.end_reason ?? null;
    cursor = page.next_cursor;
  }
  captured.next_cursor = null;
  return captured;
}

/** A recorded task is mapped again in this captured review; its old id is never an End command id. */
export function reviewedTargetRequest(roster: ThreadBackground, target: BackgroundEndTarget): BackgroundEndRequest | null {
  if ('task' in target) {
    const task = roster.wake?.tasks.find((task) => task.id === target.task.id && task.item_id === target.task.item_id && task.turn_id === target.task.turn_id);
    return task?.state === 'running' && task.can_end && typeof task.end_id === 'string' ? reviewedRequest(roster, [task.end_id]) : null;
  }
  const command = roster.items.find((command) => command.id === target.command.id && command.segment_id === target.command.segment_id && command.item_id === target.command.item_id && command.turn_id === target.command.turn_id);
  return command === undefined ? null : reviewedRequest(roster, [command.id]);
}

/** A partial, unavailable or untransferred set cannot authorize End all. */
export function canEndAll(roster: ThreadBackground): boolean {
  return endReasonInReview(roster) === null && roster.total !== null && roster.total > 0 && roster.next_cursor === null && roster.items.length === roster.total
    && roster.items.every((command) => endReasonInReview(roster, command) === null);
}

function endableInReview(roster: ThreadBackground, id: string): boolean {
  const command = roster.items.find((command) => command.id === id);
  return command !== undefined && endReasonInReview(roster, command) === null;
}

/** Native action facts stay distinct from inspection availability and from an uncaptured target. */
export function endReasonInReview(roster: ThreadBackground, command?: BackgroundCommand): BackgroundEndReason | null {
  if (roster.end_reason != null) return roster.end_reason;
  if (!roster.supported) return 'unsupported';
  if (roster.state === 'offline') return 'offline';
  if (!roster.can_end || roster.state !== 'live') return 'unavailable';
  if (roster.list_ref === null || roster.total === 0) return 'no_current_target';
  if (command === undefined) return null;
  if (command.end_reason != null) return command.end_reason;
  const task = roster.wake?.tasks.find((task) => task.end_id === command.id);
  return task === undefined || task.can_end && task.state === 'running' ? null : 'no_current_target';
}

/** Captures the request at confirmation, keeping later native work out of the accepted set. */
export function reviewedRequest(roster: ThreadBackground, ids: readonly string[] | 'all'): BackgroundEndRequest | null {
  if (endReasonInReview(roster) !== null || roster.list_ref === null) return null;
  if (ids === 'all') {
    const count = roster.total;
    return count !== null && canEndAll(roster) ? { list_ref: roster.list_ref, all: true, count } : null;
  }
  const selected = new Set(ids);
  if (selected.size === 0 || selected.size > 200 || selected.size !== ids.length || !ids.every((id) => endableInReview(roster, id))) return null;
  return { list_ref: roster.list_ref, command_ids: [...ids] };
}

/** Reads only factual mutation outcomes; no job result can become the next live roster. */
export function endResults(value: unknown): BackgroundEndResult['results'] {
  if (typeof value !== 'object' || value === null || !('results' in value) || !Array.isArray(value.results)) return [];
  return (value.results as BackgroundEndResult['results']).filter((entry) => typeof entry.id === 'string' && ['ended', 'already_ended', 'still_running', 'unknown'].includes(entry.state));
}
