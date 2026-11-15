/** Native archived and current facts share a source; the reader's view chooses which to show. */
import type { TreeFold, TreeFoldCounts } from '@/generated/api-types';
import { THREAD_URGENCY, isThreadUrgency } from '@/lib/threadDisplay';

export function visibleTreeFold(fold: TreeFold | null | undefined, showArchived: boolean): TreeFoldCounts | null {
  if (fold === null || fold === undefined) return null;
  const archived = showArchived ? fold.archived : null;
  if (archived === null || archived === undefined) return fold;
  const rank = (state: TreeFoldCounts['urgency']) => isThreadUrgency(state) ? THREAD_URGENCY.indexOf(state) : THREAD_URGENCY.length;
  return {
    descendants: fold.descendants + archived.descendants,
    children: fold.children + archived.children,
    waiting: fold.waiting + archived.waiting,
    unread: fold.unread || archived.unread,
    mentions: fold.mentions + archived.mentions,
    urgency: rank(fold.urgency) <= rank(archived.urgency) ? fold.urgency : archived.urgency,
  };
}
