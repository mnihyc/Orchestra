/** Live conversation summary and actionable approval.
 *
 * One thread of a topic as a card: its name, how it stands (running, waiting for an approval, blocked,
 * idle), how many approvals wait and when something last happened in it, from what the topic's live
 * tree says of it. What the thread said last, which model and agent it uses and the buttons of an
 * approval are the thread's own data, which the thread screen reads; the card opens it.
 */
import { useTranslation } from 'react-i18next';

import { PlaceLink } from '@/features/navigation/places';
import { attentionOf, WorkChips } from '@/features/source/Attention';
import type { TreeNode } from '@/generated/api-types';
import { Badge } from '@/ui/Badge';
import { cx } from '@/ui/cx';
import { RelativeTime } from '@/ui/RelativeTime';
import type { WorkState } from '@/ui/StateChip';
import { ThreadStateChip, threadDisplayState } from '@/ui/ThreadState';

/** The state a thread shows: its own, or archived once it is. */
export function threadState(thread: TreeNode): WorkState {
  return threadDisplayState(thread.thread?.display);
}

export function ThreadCard({ thread }: { thread: TreeNode }) {
  const { t } = useTranslation();
  const approvals = thread.thread?.pending_approvals ?? 0;
  const last = thread.thread?.last_activity_at ?? null;
  return (
    <li
      className={cx(
        'relative flex flex-col gap-3 rounded-lg border border-border bg-surface p-4 transition-colors hover:border-border-strong hover:bg-bg-subtle',
        'has-[a:focus-visible]:outline-2 has-[a:focus-visible]:outline-offset-2 has-[a:focus-visible]:outline-focus',
        thread.archived && 'opacity-60',
      )}
    >
      <div className="flex items-start gap-2">
        <PlaceLink place={thread} className="min-w-0 flex-1 truncate font-medium text-text outline-none after:absolute after:inset-0 after:rounded-lg">
          {thread.name}
        </PlaceLink>
        {approvals > 0 && <Badge tone="warning" count={approvals} label={t('navigation.approvalsWaiting', { count: approvals })} />}
      </div>
      <div className="flex flex-wrap items-center justify-between gap-2 text-sm text-text-muted">
        <ThreadStateChip display={thread.thread?.display} />
        <WorkChips attention={attentionOf(thread.thread)} compact context="card" />
        {last !== null && (
          <span>
            {t('places.cards.active')} <RelativeTime at={last} />
          </span>
        )}
      </div>
    </li>
  );
}
