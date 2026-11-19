/** What is unread and what waits, as the rail, the left bar and the bottom bar of a small screen show it.
 *
 * Two numbers, kept by the person's live feed: how many rows of the Inbox are not read, and how many
 * approvals wait for a decision they may give. Whoever shows one follows the feed, so the numbers
 * move as things happen, on every screen, and a screen reader is told what a number counts.
 */
import { useEffect } from 'react';
import { useTranslation } from 'react-i18next';

import { markNotificationsRead } from '@/features/inbox/read';
import { useUserFeed } from '@/realtime/live';
import { type Counters, useCounters, useUserStore } from '@/realtime/stores/user';
import { Badge } from '@/ui/Badge';
import { useFrameReady } from '@/shell/FrameReadiness';
import { useViewShown } from '@/features/workbench/viewOpening';

/** The counters, with the person's feed followed for as long as the caller is shown. */
export function useInboxCounters(): Counters & { loaded: boolean } {
  const ready = useFrameReady();
  useUserFeed(false, ready);
  return useCounters();
}

/** The words that say what the two numbers count, for a screen reader and a tooltip; `null` while a number is zero. */
export function useCounterWords(): { unread: string | null; approvals: string | null } {
  const { t } = useTranslation();
  const { unread, approvalsPending } = useCounters();
  return {
    unread: unread > 0 ? t('inbox.unreadCount', { count: unread }) : null,
    approvals: approvalsPending > 0 ? t('navigation.approvalsWaiting', { count: approvalsPending }) : null,
  };
}

/** A number on an entry of navigation. It says nothing for zero. */
export function CountBadge({ count, tone, label, className }: { count: number; tone: 'accent' | 'warning'; label: string | null; className?: string }) {
  if (count <= 0) return null;
  return <Badge tone={tone} count={count} {...(label === null ? {} : { label })} {...(className === undefined ? {} : { className })} />;
}


/**
 * A new notification delivered while its conversation is visible has been seen. Showing an existing
 * tab changes nothing: older notifications remain under the Inbox's explicit read controls.
 */
export function useReadWhenOpened(thread: string | null): void {
  const ready = useFrameReady();
  const shown = useViewShown();
  useEffect(() => {
    if (thread === null || !ready || !shown) return;
    return useUserStore.subscribe((next, before) => {
      if (document.visibilityState !== 'visible' || next.loaded !== before.loaded || next.lastSeq <= before.lastSeq) return;
      const ids = [...next.rows.values()].filter((row) => row.thread_id === thread && row.read_at === null
        && row.inbox_seq > before.lastSeq && !before.rows.has(row.id)).map((row) => row.id);
      if (ids.length > 0) void markNotificationsRead({ ids });
    });
  }, [thread, ready, shown]);
}
