/** Day grouping, unread and mention filters.
 *
 * What the person was told, newest first, grouped by their own days: one row for each thing, with the
 * kind it is, where it happened and when, unread ones bold and marked. Three filters (All, Unread,
 * Mentions) and "Mark all read". The rows are the person's live feed: a notification that arrives
 * appears at the top and the counters move with it. Older ones come a page at a time.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { CheckCheck, Inbox as InboxIcon } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchNotifications } from '@/api/notifications';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import { Hint } from '@/features/help/Hint';
import { useNotificationLabels } from '@/features/inbox/presentation';
import { PlaceInbox } from './away/Grouped';
import { GroupingControl, useInboxGrouping } from './away/Grouping';
import { PauseControls } from '@/features/settings/personal/away/Pause';
import { useAwayLive } from '@/features/settings/personal/away/live';
import { dayKey, FILTERS, filterRows, groupByDay, type InboxFilter, previousDay, writeDay } from '@/features/inbox/model';
import { markNotificationsRead } from '@/features/inbox/read';
import { useAppearance } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { reloadUserFeed, useUserFeed } from '@/realtime/live';
import { useHeldRows, userWriter, useUserStore } from '@/realtime/stores/user';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';

function Loading() {
  return (
    <SkeletonGroup className="flex flex-col divide-y divide-border overflow-hidden rounded-lg border border-border bg-surface">
      {Array.from({ length: 5 }, (_, index) => (
        <div key={index} className="flex items-start gap-3 px-3 py-3">
          <Skeleton className="size-8 rounded-full" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-4 w-1/2" />
            <Skeleton className="h-3 w-3/4" />
          </div>
        </div>
      ))}
    </SkeletonGroup>
  );
}

export function Inbox({ filter, onFilter }: { filter: InboxFilter; onFilter: (next: InboxFilter) => void }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  useUserFeed();
  useAwayLive();
  const grouping = useInboxGrouping();
  useScreenHeading({ title: t('screens.inbox.title'), icon: InboxIcon });

  const held = useHeldRows();
  const loaded = useUserStore((state) => state.loaded);
  const error = useUserStore((state) => state.error);
  const edge = useUserStore((state) => state.edge);
  const reachedEnd = useUserStore((state) => state.reachedEnd);
  const nextCursor = useUserStore((state) => state.nextCursor);
  const unread = useUserStore((state) => state.counters.unread);

  const rows = useMemo(() => filterRows(held, filter, { edge, reachedEnd }), [held, filter, edge, reachedEnd]);
  const labels = useNotificationLabels(rows);
  const groups = useMemo(() => groupByDay(rows, timeZone), [rows, timeZone]);
  const today = dayKey(now, timeZone);
  const yesterday = previousDay(today);

  // The pages of the newest-first order, for All and Mentions.
  const [older, setOlder] = useState<{ loading: boolean; error: unknown }>({ loading: false, error: null });
  const loadOlder = () => {
    setOlder({ loading: true, error: null });
    fetchNotifications({ cursor: nextCursor }).then(
      (page) => {
        userWriter.older(page.items, page.next_cursor);
        setOlder({ loading: false, error: null });
      },
      (failure: unknown) => setOlder({ loading: false, error: failure }),
    );
  };

  // Unread rows lie wherever they lie: the server is asked for them by themselves, beyond the pages.
  const unreadPages = useInfiniteQuery({
    queryKey: ['inbox', 'unread'] as const,
    queryFn: ({ pageParam, signal }) => fetchNotifications({ unread: true, cursor: pageParam }, signal),
    initialPageParam: null as string | null,
    getNextPageParam: (page) => page.next_cursor,
    enabled: grouping.value === 'day' && filter === 'unread' && loaded && !reachedEnd,
  });
  useEffect(() => {
    for (const page of unreadPages.data?.pages ?? []) userWriter.found(page.items);
  }, [unreadPages.data]);

  const onUnread = filter === 'unread';
  const hasMore = onUnread ? !reachedEnd && unreadPages.hasNextPage : !reachedEnd;
  const loadingMore = onUnread ? unreadPages.isFetchingNextPage : older.loading;
  const moreError = onUnread ? (unreadPages.isFetchNextPageError ? unreadPages.error : null) : older.error;
  const more = () => (onUnread ? void unreadPages.fetchNextPage() : loadOlder());

  const dayLabel = (key: string) => (key === today ? t('inbox.today') : key === yesterday ? t('inbox.yesterday') : writeDay(key, today, locale));
  const filters = FILTERS.map((value) => ({
    value,
    name: value === 'unread' && unread > 0 ? t('inbox.filter.unreadWithCount', { count: unread }) : t(`inbox.filter.${value}`),
  }));
  const empty = loaded && rows.length === 0 && !hasMore && !(onUnread && unreadPages.isFetching);

  return (
    <Page>
      <PageHeader
        mark={
          <PlaceMark>
            <InboxIcon aria-hidden />
          </PlaceMark>
        }
        title={t('screens.inbox.title')}
        actions={
          <>
            <Hint id="inbox" popover />
            <GroupingControl value={grouping.value} onChange={grouping.choose} disabled={grouping.loading || grouping.saving} error={grouping.error} />
            <SegmentedControl
              label={t('inbox.filter.label')}
              value={filter}
              onValueChange={onFilter}
              segments={filters}
            />
            <Button icon={<CheckCheck aria-hidden />} disabled={unread === 0} onClick={() => void markNotificationsRead({ all: true })}>
              {t('inbox.markAllRead')}
            </Button>
            <PauseControls />
          </>
        }
      />
      {/* A change of the count is read to somebody who cannot see it. */}
      <p role="status" className="sr-only">
        {loaded && unread > 0 ? t('inbox.unreadCount', { count: unread }) : ''}
      </p>

      {grouping.loading ? <Loading /> : grouping.value === 'place' ? <PlaceInbox filter={filter} /> : !loaded && error === null ? (
        <Loading />
      ) : !loaded ? (
        <ErrorState error={error} onRetry={reloadUserFeed} />
      ) : empty ? (
        <EmptyState icon={<InboxIcon aria-hidden />} title={t(`inbox.empty.${filter}.title`)} description={t(`inbox.empty.${filter}.description`)} />
      ) : (
        <div className="flex flex-col gap-4">
          {groups.map((group) => (
            <section key={group.key} aria-labelledby={`inbox-day-${group.key}`} className="flex flex-col gap-2">
              <h2 id={`inbox-day-${group.key}`} className="px-1 text-sm font-medium text-text-muted">
                {dayLabel(group.key)}
              </h2>
              <ul className="flex flex-col divide-y divide-border overflow-hidden rounded-lg border border-border bg-surface">
                {group.rows.map((row) => (
                  <NotificationRow key={row.id} row={row} labels={labels} />
                ))}
              </ul>
            </section>
          ))}
          {rows.length === 0 && <EmptyState compact className="px-1" title={t('inbox.noneYet')} />}
          <Pagination hasMore={hasMore} loading={loadingMore} error={moreError} onMore={more} />
        </div>
      )}
      {loaded && error !== null && <ErrorState compact error={error} onRetry={reloadUserFeed} />}
    </Page>
  );
}
