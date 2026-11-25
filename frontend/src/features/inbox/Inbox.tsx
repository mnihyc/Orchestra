/** Day grouping, unread and mention filters.
 *
 * What the person was told, newest first, grouped by their own days: one row for each thing, with the
 * kind it is, where it happened and when, unread ones bold and marked. Three filters (All, Unread,
 * Mentions) and "Mark all read". The rows are the person's live feed: a notification that arrives
 * appears at the top and the counters move with it. Older ones come a page at a time.
 */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { CheckCheck, Inbox as InboxIcon } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchNotifications } from '@/api/notifications';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import { InboxRetention } from './Retention';
import { Hint } from '@/features/help/Hint';
import { useNotificationLabels } from '@/features/inbox/presentation';
import { PlaceInbox } from './away/Grouped';
import { GroupingControl, useInboxGrouping } from './away/Grouping';
import { PauseControls } from '@/features/settings/personal/away/Pause';
import { useAwayLive } from '@/features/settings/personal/away/live';
import { dayKey, FILTERS, filterRows, groupByDay, groupByKind, type InboxFilter, previousDay, writeDay } from '@/features/inbox/model';
import { lookOf } from '@/features/inbox/kinds';
import { markNotificationsRead } from '@/features/inbox/read';
import { useAppearance } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { reloadUserFeed, useUserFeed } from '@/realtime/live';
import { useHeldRows, userWriter, useUserStore } from '@/realtime/stores/user';
import { Page, PageHeader, PlaceMark } from '@/shell/Page';
import { useScreenHeading } from '@/shell/screen';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { ScrollRegion } from '@/ui/ScrollRegion';
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
  const search=useCollectionSearch('inbox');
  const cache=useQueryClient();
  const searching=search.q!=='';
  const searchKey=['inbox-search',search.scope,filter,search.q] as const;
  const matching=useInfiniteQuery({queryKey:searchKey,initialPageParam:null as string|null,
    queryFn:({pageParam,signal})=>fetchNotifications({q:search.q,cursor:pageParam,...(filter==='unread'?{unread:true}:{}),...(filter==='mentions'?{kind:'mention,doc.mention,person.notify'}:{})},signal),
    getNextPageParam:(page)=>page.next_cursor,enabled:searching && grouping.value!=='place'});
  const sequence=useUserStore((state)=>state.lastSeq);
  const priorSequence=useRef(sequence);
  useEffect(()=>{if(priorSequence.current!==sequence){priorSequence.current=sequence;void cache.resetQueries({queryKey:['inbox-search',search.scope]});}},[sequence,cache,search.scope]);
  useScreenHeading({ title: t('screens.inbox.title'), icon: InboxIcon });

  const held = useHeldRows();
  const loaded = useUserStore((state) => state.loaded);
  const error = useUserStore((state) => state.error);
  const edge = useUserStore((state) => state.edge);
  const reachedEnd = useUserStore((state) => state.reachedEnd);
  const nextCursor = useUserStore((state) => state.nextCursor);
  const unread = useUserStore((state) => state.counters.unread);

  const rows = useMemo(() => searching ? matching.data?.pages.flatMap((page)=>page.items) ?? [] : filterRows(held, filter, { edge, reachedEnd }), [searching,matching.data,held, filter, edge, reachedEnd]);
  const labels = useNotificationLabels(rows);
  const groups = useMemo(() => grouping.value === 'kind' ? groupByKind(rows) : groupByDay(rows, timeZone), [rows, timeZone, grouping.value]);
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
    enabled: grouping.value !== 'place' && filter === 'unread' && !searching && loaded && !reachedEnd,
  });
  useEffect(() => {
    for (const page of unreadPages.data?.pages ?? []) userWriter.found(page.items);
  }, [unreadPages.data]);

  const onUnread = filter === 'unread';
  const hasMore = searching ? matching.hasNextPage : onUnread ? !reachedEnd && unreadPages.hasNextPage : !reachedEnd;
  const loadingMore = searching ? matching.isFetchingNextPage : onUnread ? unreadPages.isFetchingNextPage : older.loading;
  const moreError = searching ? matching.error : onUnread ? (unreadPages.isFetchNextPageError ? unreadPages.error : null) : older.error;
  const more = () => (searching ? void matching.fetchNextPage() : onUnread ? void unreadPages.fetchNextPage() : loadOlder());

  const dayLabel = (key: string) => (key === today ? t('inbox.today') : key === yesterday ? t('inbox.yesterday') : writeDay(key, today, locale));
  const groupLabel = (key: string) => {
    if (grouping.value !== 'kind') return dayLabel(key);
    const label = lookOf(key).label;
    return label === null ? t('inbox.otherKinds') : t(label);
  };
  const filters = FILTERS.map((value) => ({
    value,
    name: value === 'unread' && unread > 0 ? t('inbox.filter.unreadWithCount', { count: unread }) : t(`inbox.filter.${value}`),
  }));
  const ready=searching ? matching.data!==undefined : loaded;
  const listError=searching ? matching.error : error;
  const empty = ready && rows.length === 0 && !hasMore && !(onUnread && !searching && unreadPages.isFetching);

  return (
    <Page>
      <PageHeader
        mark={
          <PlaceMark>
            <InboxIcon aria-hidden />
          </PlaceMark>
        }
        title={t('screens.inbox.title')}
        help={<Hint id="inbox" popover />}
        actions={
          <>
            <GroupingControl value={grouping.value} onChange={grouping.choose} onRetry={grouping.retry} disabled={grouping.loading || grouping.saving} error={grouping.error} />
            <SegmentedControl
              label={t('inbox.filter.label')}
              value={filter}
              onValueChange={onFilter}
              segments={filters}
            />
            <Button icon={<CheckCheck aria-hidden />} disabled={unread === 0} reason={unread === 0 ? loaded ? t('ui.unavailable.noUnread') : t('common.loading') : undefined} onClick={() => void markNotificationsRead({ all: true })}>
              {t('inbox.markAllRead')}
            </Button>
            <PauseControls />
          </>
        }
      />
      <CollectionSearch name={t('screens.inbox.title')} {...search}/>
      {/* A change of the count is read to somebody who cannot see it. */}
      <p role="status" className="sr-only">
        {loaded && unread > 0 ? t('inbox.unreadCount', { count: unread }) : ''}
      </p>

      <ScrollRegion kind="table" label={t('screens.inbox.title')} className="max-h-[70dvh] overflow-y-auto">
      {grouping.loading ? <Loading /> : grouping.value === 'place' ? <PlaceInbox filter={filter} q={search.q} /> : !ready && listError === null ? (
        <Loading />
      ) : !ready ? (
        <ErrorState error={listError} onRetry={searching?()=>void matching.refetch():reloadUserFeed} />
      ) : empty ? (
        <EmptyState icon={<InboxIcon aria-hidden />} title={t(searching?'ui.collection.none':`inbox.empty.${filter}.title`)} description={searching?undefined:t(`inbox.empty.${filter}.description`)} />
      ) : (
        <div className="flex flex-col gap-4">
          {groups.map((group) => (
            <section key={group.key} aria-labelledby={`inbox-${grouping.value}-${group.key}`} className="flex flex-col gap-2">
              <h2 id={`inbox-${grouping.value}-${group.key}`} className="px-1 text-sm font-medium text-text-muted">
                {groupLabel(group.key)}
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
      </ScrollRegion>
      {loaded && <div className="flex min-w-0 justify-end"><InboxRetention/></div>}
      {loaded && error !== null && <ErrorState compact error={error} onRetry={reloadUserFeed} />}
    </Page>
  );
}
