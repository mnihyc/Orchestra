import { useInfiniteQuery } from '@tanstack/react-query';
import { topicEnvironmentsPageQuery } from '@/api/topics';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { Table, type Column } from '@/ui/Table';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
/** The environments this topic's threads use: owner, place, threads bound, mounts, the default for new threads, and adding one.
 *
 * The environments the threads of a topic run in, and those made for it, as the topic names them: each
 * with its name, its owner, where it comes from, how it stands and whether it is the one new threads
 * join. The topic owns none of them; each belongs to the person who made it. What is only known to the
 * environment itself (its mounts, how long it stays awake, which threads are in it) is on its own page,
 * which each name leads to.
 */
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import type { EnvironmentRef, Topic } from '@/generated/api-types';
import { ResourceName } from '@/features/resource-facts/ResourceFacts';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { UserChip } from '@/ui/UserChip';

/** How many environments a row of a list draws a dot for, before it counts the rest. */
const DOTS_SHOWN = 3;

/** The environments of a topic as one dot each, up to three and then how many more. */
export function EnvironmentDots({ environments, className }: { environments: readonly EnvironmentRef[]; className?: string }) {
  const { t } = useTranslation();
  if (environments.length === 0) return null;
  return (
    <span className={cx('inline-flex items-center', className)}>
      {environments.slice(0, DOTS_SHOWN).map((environment) => (
        <ResourceName key={environment.id} subject={{ kind: 'environment', environment }} hint={false}><EnvironmentDot status={environment.status} name={environment.name} /></ResourceName>
      ))}
      {environments.length > DOTS_SHOWN && (
        <span className="ml-1 text-sm text-text-muted">{t('places.environments.more', { count: environments.length - DOTS_SHOWN })}</span>
      )}
    </span>
  );
}

/** Overview and the full tab share the same native selector and bounded rows. */
export function TopicEnvironments({ topic, compact = false }: { topic: Pick<Topic, 'id' | 'environments' | 'default_environment_id'>; compact?: boolean }) {
  const { t } = useTranslation();
  const search = useCollectionSearch('topic-environments:' + topic.id);
  const choices = topicEnvironmentsPageQuery(topic.id, search.q);
  const snapshot = topic.environments;
  const query = useInfiniteQuery({ ...choices,
    queryKey: [...choices.queryKey, search.q === '' && snapshot.length <= 50 ? JSON.stringify([snapshot, topic.default_environment_id]) : null],
    // This complete small native topic projection is already admitted; searches always use the server.
    ...(search.q === '' && snapshot.length <= 50 ? { initialData: { pages: [{ items: snapshot, next_cursor: null }], pageParams: [null] }, staleTime: Infinity } : {}),
  });
  const rows = query.data?.pages.flatMap((page) => page.items) ?? [];
  const columns: Column<EnvironmentRef>[] = [
    { id: 'name', header: t('resources.listName'), cell: (environment) => <span className="flex min-w-0 flex-col gap-1"><ResourceName subject={{ kind: 'environment', environment }} asChild><Link to="/resources/environments/$id" params={{ id: environment.id }} className="truncate font-medium hover:underline">{environment.name}</Link></ResourceName>{environment.id === topic.default_environment_id && <span className="text-sm text-text-muted">{t('places.environments.default')}</span>}</span> },
    { id: 'state', header: t('hosts.runtimes.state'), cell: (environment) => <EnvironmentDot status={environment.status} showLabel /> },
    ...(!compact ? [{ id: 'owner', header: t('places.topic.owner'), from: 'md' as const, cell: (environment: EnvironmentRef) => <UserChip user={environment.owner} size={24} thing={{ kind: 'environment', id: environment.id }} /> }] : []),
  ];
  return <div className="flex min-w-0 flex-col gap-3">
    <CollectionSearch name={t('places.topic.environments')} {...search} />
    {query.isPending ? <SkeletonRows read={query} rows={3} /> : query.isError && !query.isFetchNextPageError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('places.environments.empty')} /> : <Table label={t('places.topic.environments')} columns={columns} rows={rows} rowKey={(row) => row.id} virtual windowKey={topic.id + search.q} />}
    <Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} />
  </div>;
}
