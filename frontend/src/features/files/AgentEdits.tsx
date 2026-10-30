/** A file that is open following the agent's writing: the changed lines marked until the next turn, and who changed it.
 *
 * A tab without unsaved text follows the file: when it changes, the new text replaces the old and
 * the lines that differ are marked in the margin until the next change. Who changed it is read from
 * the changes the threads' tool calls recorded: when a turn of this thread wrote the file a moment
 * ago, the line under the tab says so.
 */
import { useQuery } from '@tanstack/react-query';

import { changesQuery, type Door } from '@/api/workspace';

/** The most lines that are marked as changed: a file that was written afresh is not lit from end to end. */
export const MARKED_AT_MOST = 400;

/**
 * The lines of `after` (counted from one) that are not in `before`, found as what lies between the
 * lines the two share at their beginning and at their end. It is the part that was written, said
 * without a comparison that would cost more than it shows; a change in two places marks what is between them too.
 */
export function changedLines(before: string, after: string): number[] {
  if (before === after) return [];
  const old = before.split('\n');
  const next = after.split('\n');
  let start = 0;
  while (start < old.length && start < next.length && old[start] === next[start]) start += 1;
  let oldEnd = old.length;
  let nextEnd = next.length;
  while (oldEnd > start && nextEnd > start && old[oldEnd - 1] === next[nextEnd - 1]) {
    oldEnd -= 1;
    nextEnd -= 1;
  }
  const lines: number[] = [];
  // Lines were taken away and none were put in: the line where they were is lit, so that the place is not lost.
  if (nextEnd === start) return start < next.length ? [start + 1] : next.length > 0 ? [next.length] : [];
  for (let index = start; index < nextEnd && lines.length < MARKED_AT_MOST; index += 1) lines.push(index + 1);
  return lines;
}

/** How recent a recorded change must be to be named as what changed the file under the person: a turn that wrote it a moment ago. */
export const RECENT_CHANGE_MS = 2 * 60_000;

/** Who changed a file just now, from the changes the tool calls recorded: the thread, and so the agent it works with. */
export function useWhoChanged(door: Door, path: string, since: number | null): { threadId: string } | null {
  const query = useQuery({ ...changesQuery(door, true), enabled: since !== null });
  if (since === null || query.data === undefined) return null;
  const found = query.data.find((change) => change.path === path && Math.abs(Date.parse(change.at) - since) < RECENT_CHANGE_MS);
  return found === undefined ? null : { threadId: found.thread_id };
}
