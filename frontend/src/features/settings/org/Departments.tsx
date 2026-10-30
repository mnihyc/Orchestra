/** The orgs that lie directly in this one, at whatever level: making one, its short name, how much each holds, and its own settings, which are these same sections; not shown for an org that holds none.
 *
 * A department is an org with its own members, groups, tags and policies, and departments of its own,
 * as deep as the team needs. Each row names one and opens its settings, which read the same as these.
 * Making one is for whoever manages the org; what it takes is said when the reader is not one.
 */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { LayoutGrid, Settings } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { childrenQuery } from '@/api/nodes';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { placeOfNode } from '@/features/navigation/places';
import { takes } from '@/features/settings/refusals';
import type { Org } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { PermissionGate } from '@/ui/PermissionGate';
import { SkeletonRows } from '@/ui/Skeleton';
import { Table, type Column } from '@/ui/Table';

export function Departments({ org }: { org: Org }) {
  const { t } = useTranslation();
  const list = useInfiniteQuery(childrenQuery(org.id));
  const departments = (list.data?.pages.flatMap((page) => page.items) ?? []).filter((node): node is Org => node.kind === 'org');
  const may = holds(org.access, 'org.manage');

  const columns: Column<Org>[] = [
    {
      id: 'name',
      header: t('departments.columns.name'),
      cell: (department) => (
        <span className="flex flex-col">
          <Link
            to="/org/$slug/settings/$section"
            params={{ slug: department.address, section: 'general' }}
            className="font-medium wrap-anywhere text-text underline-offset-2 hover:underline"
          >
            {department.name}
          </Link>
        </span>
      ),
    },
    { id: 'short', header: t('departments.columns.shortName'), from: 'sm', cell: (department) => <span className="font-mono">{department.slug}</span> },
    { id: 'orgs', header: t('departments.columns.departments'), from: 'md', cell: (department) => department.org_count },
    { id: 'projects', header: t('departments.columns.projects'), from: 'md', cell: (department) => department.project_count },
    { id: 'people', header: t('departments.columns.people'), from: 'sm', cell: (department) => department.member_count },
    {
      id: 'open',
      header: t('departments.open'),
      hideHeader: true,
      align: 'end',
      cell: (department) => (
        <Button size="sm" variant="ghost" asChild>
          <Link to="/org/$slug/settings/$section" params={{ slug: department.address, section: 'general' }} aria-label={t('departments.openSettings', { name: department.name })}>
            <Settings aria-hidden />
            {t('departments.openSettingsShort')}
          </Link>
        </Button>
      ),
    },
  ];

  return (
    <Section
      id="departments"
      title={t('settings.org.sectionNames.departments')}
      lead={t('departments.lead', { name: org.name })}
      action={
        <PermissionGate allowed={may} mode="block" reason={takes(t, 'org.manage', org.name, org.owner)}>
          <Button size="sm" variant="primary" icon={<LayoutGrid aria-hidden />} onClick={() => openNodeDialog({ type: 'new-department', parent: placeOfNode(org) })}>
            {t('departments.new')}
          </Button>
        </PermissionGate>
      }
    >
      {list.isPending ? (
        <SkeletonRows rows={3} />
      ) : list.isError && departments.length === 0 ? (
        <ErrorState compact error={list.error} retrying={list.isRefetching} onRetry={() => void list.refetch()} />
      ) : departments.length === 0 ? (
        <EmptyState compact icon={<LayoutGrid aria-hidden />} title={t('departments.none', { name: org.name })} description={may ? t('departments.noneAbout') : undefined} />
      ) : (
        <div className="flex flex-col gap-2">
          <Table label={t('departments.table', { name: org.name })} columns={columns} rows={departments} rowKey={(department) => department.id} />
          <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
        </div>
      )}
      <p className="text-sm text-text-muted">{t('departments.depth')}</p>
    </Section>
  );
}
