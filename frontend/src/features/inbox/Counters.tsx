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
 * Opening a thread marks what the person was told about it read, and so does what arrives while they
 * look at it: they are looking at it. A tab that is hidden is not looking.
 */
export function useReadWhenOpened(thread: string | null): void {
  const rows = useUserStore((state) => state.rows);
  const loaded = useUserStore((state) => state.loaded);
  useEffect(() => {
    if (thread === null || !loaded) return;
    const mark = () => {
      if (document.visibilityState === 'visible') void markNotificationsRead({ place: thread }, { ask: true });
    };
    mark();
    // Back in front: what came while the tab was hidden was not seen.
    document.addEventListener('visibilitychange', mark);
    return () => document.removeEventListener('visibilitychange', mark);
  }, [thread, loaded]);
  useEffect(() => {
    // A notification that arrives while the thread is open is read at once.
    if (thread !== null && loaded && document.visibilityState === 'visible') void markNotificationsRead({ place: thread });
  }, [thread, loaded, rows]);
}
