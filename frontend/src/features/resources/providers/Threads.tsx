/** Visible bound threads and batch switch.
 *
 * The threads bound to the provider that the reader may view, of whatever org, each with its topic, its
 * org where that is another than the provider's, its owner and its state, a page at a time; and how many
 * more there are than the reader may see. The shared batch switch selects all current bindings the
 * caller may switch, including when the source provider is switched off.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ArrowRightLeft, MessagesSquare } from 'lucide-react';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { providerThreadsQuery } from '@/api/resources';
import { openChange } from '@/features/change/target';
import { TopicPlace } from '@/features/navigation/PlaceChain';
import type { Provider, ProviderThread } from '@/generated/api-types';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { ThreadStateChip } from '@/ui/ThreadState';
import { type Column, Table } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

export function ProviderThreads({ provider }: { provider: Provider }) {
  const { t } = useTranslation();
  const list = useInfiniteQuery(providerThreadsQuery(provider.id));
  const items = useMemo(() => list.data?.pages.flatMap((page) => page.items) ?? [], [list.data]);
  const hidden = list.data?.pages[0]?.hidden ?? 0;

  const columns: Column<ProviderThread>[] = [
    {
      id: 'thread',
      header: t('providers.threads.thread'),
      cell: (row, shown) => {
        const withoutTopic = !shown.has('topic');
        const withoutOwner = !shown.has('owner');
        return (
          <div className="flex min-w-0 flex-col gap-1">
            <Link to="/thread/$threadId" params={{ threadId: row.thread.id }} className="truncate font-medium text-text hover:underline">
              {row.thread.name ?? t('providers.threads.hiddenName')}
            </Link>
            {withoutTopic && (
              <>
                <Link to="/topic/$topicId" params={{ topicId: row.topic.id }} className="truncate text-sm text-text-muted hover:underline">
                  {row.topic.name ?? ''}
                </Link>
                <TopicPlace topic={row.topic} className="text-sm" />
              </>
            )}
            {withoutOwner && <UserChip user={row.owner} size={24} className="text-sm" />}
          </div>
        );
      },
    },
    {
      id: 'topic',
      header: t('providers.threads.topic'),
      from: 'md',
      cell: (row) => (
        <span className="flex min-w-0 flex-col">
          <Link to="/topic/$topicId" params={{ topicId: row.topic.id }} className="truncate text-text hover:underline">
            {row.topic.name ?? ''}
          </Link>
          <TopicPlace topic={row.topic} className="text-sm" />
          {row.org.id !== provider.home.id && <span className="truncate text-sm text-text-muted">{t('providers.threads.inOrg', { org: row.org.name ?? '' })}</span>}
        </span>
      ),
    },
    { id: 'owner', header: t('providers.threads.owner'), from: 'md', cell: (row) => <UserChip user={row.owner} size={24} /> },
    {
      id: 'state',
      header: t('providers.threads.state'),
      cell: (row) => <ThreadStateChip display={row.thread.thread_display} />,
    },
  ];

  return (
    <Section
      id="provider-threads"
      title={t('providers.threads.title')}
      lead={t('providers.threads.lead')}
      action={
        items.length > 0 ? (
          <Button icon={<ArrowRightLeft aria-hidden />} onClick={() => openChange({ kind: 'provider', id: provider.id, name: provider.name }, { kind: 'switch' })}>
            {t('change.menu.switchThreads')}
          </Button>
        ) : undefined
      }
    >
      {list.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError && items.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : items.length === 0 && hidden === 0 ? (
        <EmptyState compact icon={<MessagesSquare aria-hidden />} title={t('providers.threads.none')} />
      ) : (
        <div className="flex flex-col gap-2">
          {items.length > 0 && <Table label={t('providers.threads.title')} columns={columns} rows={items} rowKey={(row) => row.thread.id} />}
          <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
          {hidden > 0 && <p className="text-sm text-text-muted">{t('providers.threads.hidden', { count: hidden })}</p>}
        </div>
      )}
    </Section>
  );
}
