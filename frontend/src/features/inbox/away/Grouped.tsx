/** Current readable place groups, actionable rows first, with paged identity merging and explicit read/Undo. */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { CheckCheck, ExternalLink, Inbox as InboxIcon } from 'lucide-react';
import { useEffect, useMemo, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchPlacePage, groupedKey } from '@/api/away';
import { ApiError } from '@/api/client';
import { NotificationRow } from '@/features/inbox/NotificationRow';
import { useNotificationLabels } from '@/features/inbox/presentation';
import type { InboxFilter } from '@/features/inbox/model';
import { useUserStore } from '@/realtime/stores/user';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { ReadReceipt, useGroupReads } from './Reads';
import { mergePlacePages, placeUrl } from './model';

export function PlaceInbox({ filter }: { filter: InboxFilter }) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const seq = useUserStore((state) => state.lastSeq);
  const before = useRef(seq);
  const key = useMemo(() => [...groupedKey, filter], [filter]);
  const query = useInfiniteQuery({ queryKey: key, queryFn: ({ pageParam, signal }) => fetchPlacePage(filter, pageParam, signal), initialPageParam: null as string | null, getNextPageParam: (page) => page.next_cursor, retry: false });
  // An arrival or a complete access/decision repair starts again at a current cut; old pages cannot lend headings or counts.
  useEffect(() => { if (before.current !== seq) { before.current = seq; void client.resetQueries({ queryKey: groupedKey }); } }, [seq, client]);
  const groups = useMemo(() => mergePlacePages(query.data?.pages ?? []), [query.data]);
  const rows = useMemo(() => groups.flatMap((group) => group.rows), [groups]);
  const labels = useNotificationLabels(rows);
  const resync = query.error instanceof ApiError && query.error.problem.resync === true;
  useEffect(() => { if (resync) void client.resetQueries({ queryKey:key }); }, [resync,client,key]);
  const reads = useGroupReads();
  const denied = query.error instanceof ApiError && (query.error.status === 403 || query.error.status === 404);
  if (denied || query.data === undefined && query.isError) return <ErrorState error={query.error} onRetry={() => void query.refetch()} />;
  return <div className="flex flex-col gap-3">
    {reads.acts.map((act) => <ReadReceipt key={act.key} act={act} onRetry={() => reads.retry(act)} onUndo={() => reads.undo(act)} />)}
    {query.isPending ? <SkeletonGroup className="flex flex-col gap-3">{[0, 1, 2].map((id) => <div key={id} className="rounded-lg border border-border p-4"><Skeleton className="mb-3 h-5 w-1/3" /><Skeleton className="h-12 w-full" /></div>)}</SkeletonGroup> : groups.length === 0 ? <EmptyState icon={<InboxIcon aria-hidden />} title={t(filter === 'unread' ? 'away.inbox.nothingUnread' : 'away.inbox.empty')} /> : groups.map((group) => {
      const name = group.place?.name ?? t('away.inbox.other');
      const url = placeUrl(group.place);
      const act = reads.acts.findLast((one) => one.groupKey === group.key);
      const pending = act?.phase === 'reading';
      const quiet = (act?.phase === 'reading' || act?.phase === 'unconfirmed') && act.undoKey === null;
      return <section key={group.key} aria-label={name} className="overflow-hidden rounded-lg border border-border bg-surface" data-place-group={group.key}>
        <div className="flex flex-wrap items-center gap-2 border-b border-border bg-bg-subtle p-3">
          <h2 className="min-w-0 flex-1 text-base font-medium text-text wrap-anywhere">{name}</h2>
          <span className="text-xs text-text-muted">{t('away.inbox.unread', { count: quiet ? 0 : group.unread })}</span>
          <div className="flex w-full flex-wrap gap-2 sm:w-auto">
            {url !== null && <Button size="sm" asChild><a href={url}><ExternalLink aria-hidden />{t('away.inbox.open')}</a></Button>}
            <Button size="sm" icon={<CheckCheck aria-hidden />} pending={pending} disabled={group.unread === 0 && !pending} onClick={() => reads.read(group, name)}>{t('away.inbox.mark')}</Button>
          </div>
        </div>
        <ul className="divide-y divide-border">{group.rows.map((row) => <NotificationRow key={row.id} row={quiet && row.inbox_seq <= act.through ? { ...row, read_at: row.read_at ?? new Date().toISOString() } : row} byPlace labels={labels} />)}</ul>
      </section>;
    })}
    {query.isError && !query.isFetchNextPageError && <ErrorState compact error={query.error} onRetry={() => void query.refetch()} />}
    <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />
  </div>;
}
