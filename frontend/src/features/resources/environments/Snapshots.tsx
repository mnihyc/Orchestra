/** Actual threadless workspace exports are listed by their environment's native custody reader. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { meQuery } from '@/api/auth';
import { environmentSnapshotsQuery } from '@/api/adminReach';
import { useAdminMode } from '@/features/access/AdminMode';
import type { Snapshot } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { AsAdminMark } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusChip } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';
import { Tooltip } from '@/ui/Tooltip';
import { SnapshotDownloadButton } from './WorkspaceExport';

export function EnvironmentSnapshots({ id }: { id: string }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  return <details open={open} onToggle={(event) => setOpen(event.currentTarget.open)} className="shrink-0 border-t border-border pt-2"><summary className="focus-inset cursor-pointer rounded-md text-base font-medium">{t('adminArtifacts.snapshots')}</summary>
    {open && <div className="max-h-[50dvh] overflow-auto pt-2"><SnapshotRows id={id} /></div>}
  </details>;
}
function SnapshotRows({ id }: { id: string }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const me = useQuery(meQuery), mode = useAdminMode();
  const admin = me.data?.is_system_admin !== false && mode.data?.as_admin === true;
  const list = useInfiniteQuery(environmentSnapshotsQuery(id, admin));
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const columns: Column<Snapshot>[] = [
    { id: 'name', header: t('storage.name'), grow: true, cell: (row, shown) => <div className="flex min-w-0 flex-col items-start gap-1">
      <Tooltip content={row.label || t('storage.kinds.snapshot')}><span tabIndex={0} className="max-w-full truncate font-medium">{row.label || t('storage.kinds.snapshot')}</span></Tooltip>
      <span className="flex flex-wrap items-center gap-1 text-sm text-text-muted"><StatusChip tone={row.state === 'complete' ? 'success' : row.state === 'failed' ? 'danger' : row.state === 'partial' || row.state === 'unavailable' || row.state === 'expired' ? 'warning' : 'neutral'}>{t(`adminArtifacts.snapshotStates.${row.state}`)}</StatusChip>{row.admin_access !== undefined && row.admin_access !== null && <AsAdminMark of={row.admin_access.place} />}</span>
      {row.missing_count > 0 && <span className="text-sm text-warning-text">{t('adminArtifacts.missingFiles', { number: formatNumber(row.missing_count, locale) })}</span>}
      {!shown.has('maker') && (row.created_by === null ? <span className="text-sm text-text-muted">–</span> : <UserChip user={row.created_by} />)}
      {!shown.has('size') && <span className="text-sm text-text-muted">{formatBytes(row.total_bytes, locale)}</span>}
      {!shown.has('created') && <RelativeTime at={row.created_at} className="text-sm text-text-muted" />}
    </div> },
    { id: 'maker', header: t('storage.owner'), from: 'sm', cell: (row) => row.created_by === null ? '–' : <UserChip user={row.created_by} /> },
    { id: 'size', header: t('storage.size'), from: 'md', cell: (row) => formatBytes(row.total_bytes, locale) },
    { id: 'created', header: t('adminCredentials.created'), from: 'lg', cell: (row) => <RelativeTime at={row.created_at} /> },
    { id: 'act', header: t('temporary.download'), hideHeader: true, align: 'end', cell: (row) => row.state === 'complete' || row.state === 'partial' ? <SnapshotDownloadButton id={row.id} admin={row.admin_access !== undefined && row.admin_access !== null} name={row.label || t('storage.kinds.snapshot')} /> : null },
  ];
  return <section className="flex min-w-0 flex-col gap-3" aria-label={t('adminArtifacts.snapshots')}><div className="flex flex-wrap items-center justify-between gap-2"><span /><Button size="sm" disabled={list.isFetching} onClick={() => void list.refetch()}>{t('operationKeys.activity.refresh')}</Button></div>
    {list.isPending ? <SkeletonRows read={list} rows={2} /> : list.isError && !list.isFetchNextPageError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} />
      : rows.length === 0 ? <EmptyState compact title={t('adminArtifacts.noSnapshots')} /> : <Table label={t('adminArtifacts.snapshots')} rows={rows} rowKey={(row) => row.id} columns={columns} />}
    <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
  </section>;
}
