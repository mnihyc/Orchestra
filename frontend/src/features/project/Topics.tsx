/** Permission-filtered project topics and a selection review which survives rows leaving the current filter. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { FolderOpen, SearchX } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { topicsQuery } from '@/api/nodes';
import { CreationPermissionLink } from '@/features/help/CreationPermissionLink';
import { filtersOf, isFiltered, type ProjectSearch } from '@/features/project/Project';
import { selectRange } from '@/features/project/selection';
import { TopicBoard } from '@/features/project/TopicBoard';
import { TopicList } from '@/features/project/TopicList';
import { TopicSelection } from '@/features/project/TopicSelection';
import type { Project } from '@/generated/api-types';
import { canCreateIn } from '@/lib/permissions';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';

/** A board requests the server's maximum page size; paging and figures still count only loaded, readable rows. */
const BOARD_PAGE_SIZE = 200;

export function Topics({ project, search }: { project: Project; search: ProjectSearch; onSearch: (patch: Partial<ProjectSearch>) => void }) {
  const { t } = useTranslation();
  const view = search.view ?? 'list';
  const filters = useMemo(() => filtersOf(search, view), [search, view]);
  const list = useInfiniteQuery(topicsQuery(project.id, filters, view === 'board' ? BOARD_PAGE_SIZE : undefined));
  const topics = useMemo(() => list.data?.pages.flatMap((page) => page.items) ?? [], [list.data]);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const mayOpen = canCreateIn('project', project.access);
  const content = () => {
    if (list.isPending) return <SkeletonRows rows={5} />;
    if (list.isError) return <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />;
    if (topics.length === 0) return isFiltered(search)
      ? <EmptyState icon={<SearchX aria-hidden />} title={t('places.project.noMatch')} description={t('places.project.noMatchAbout')} />
      : <EmptyState icon={<FolderOpen aria-hidden />} title={t('places.project.empty.title')} description={mayOpen ? t('places.project.empty.about') : t('places.project.empty.noRight')} action={mayOpen ? undefined : <CreationPermissionLink place={{ kind: 'project', id: project.id, name: project.name }} />} />;
    return <>
      {view === 'board' ? <TopicBoard topics={topics} canSetStatus orgId={project.org_id} showPaths={search.with_folders === true} /> : <TopicList topics={topics} showPaths={search.with_folders === true} selection={{ selected,
        onPage: (on) => setSelected((before) => { const next = new Set(before); for (const topic of topics) { if (on) next.add(topic.id); else next.delete(topic.id); } return next; }),
        toggle: (id, on, shift) => { setSelected((before) => selectRange(before, topics.map((topic) => topic.id), anchor, id, on, shift)); setAnchor(id); } }} />}
      <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} onMore={() => void list.fetchNextPage()} />
    </>;
  };
  return <div className="flex flex-col gap-3" data-project-topics>
    {view === 'list' && <TopicSelection selected={topics.filter((topic) => selected.has(topic.id))} onClear={() => setSelected(new Set())} />}
    {content()}
  </div>;
}
