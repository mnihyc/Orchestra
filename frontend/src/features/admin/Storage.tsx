/** Blob capacity and consumers.
 *
 * What the blob store holds: how many blobs, how much they come to, how much of that is kept for good
 * and how much is cache that can be made again, how much room the disk has left, and the environments
 * whose caches and pinned files take the most.
 */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { HardDrive } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { storageQuery } from '@/api/admin';
import { Figure, FIGURE_GRID } from '@/features/admin/Figure';
import type { StorageStats } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { boxClasses, Section } from '@/shell/Page';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { type Column, Table } from '@/ui/Table';
import { UserChip } from '@/ui/UserChip';

type Consumer = StorageStats['top_environments'][number];

export function Storage() {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const storage = useQuery(storageQuery);

  if (storage.isPending) {
    return (
      <div className="@container"><SkeletonGroup className={FIGURE_GRID}>
        {[0, 1, 2, 3, 4].map((at) => (
          <Skeleton key={at} className="h-16 w-full" />
        ))}
      </SkeletonGroup></div>
    );
  }
  if (storage.isError) return <ErrorState error={storage.error} retrying={storage.isRefetching} onRetry={() => void storage.refetch()} />;

  const stats = storage.data;
  const bytes = (value: number) => formatBytes(value, locale);
  // How much of the room that blobs and free space share the blobs take.
  const room = stats.blob_bytes + stats.disk_free_bytes;
  const share = room > 0 ? (stats.blob_bytes / room) * 100 : 0;

  const columns: Column<Consumer>[] = [
    {
      id: 'environment',
      header: t('admin.storage.environment'),
      cell: (row) => (
        <span className="flex min-w-0 flex-col gap-1">
          <Link to="/resources/environments/$id" params={{ id: row.environment.id }} className="w-fit font-medium text-accent-text hover:underline">
            {row.environment.name}
          </Link>
          <UserChip user={row.environment.owner} size={24} />
        </span>
      ),
    },
    { id: 'cache', header: t('admin.storage.cache'), align: 'end', cell: (row) => <span className="tabular-nums">{bytes(row.cache_bytes)}</span> },
    { id: 'pinned', header: t('admin.storage.pinned'), align: 'end', from: 'sm', cell: (row) => <span className="tabular-nums">{bytes(row.pinned_bytes)}</span> },
    { id: 'total', header: t('admin.storage.total'), align: 'end', from: 'md', cell: (row) => <span className="tabular-nums">{bytes(row.cache_bytes + row.pinned_bytes)}</span> },
  ];

  return (
    <div className="@container flex min-w-0 flex-col gap-6">
      <Section id="admin-storage-totals" title={t('admin.storage.totalsTitle')} lead={t('admin.storage.totalsLead')}>
        <ul className={FIGURE_GRID}>
          <Figure label={t('admin.storage.blobs')} value={formatNumber(stats.blob_count, locale)} />
          <Figure label={t('admin.storage.stored')} value={bytes(stats.blob_bytes)} />
          <Figure label={t('admin.storage.pinned')} value={bytes(stats.pinned_bytes)} hint={t('admin.storage.pinnedHint')} />
          <Figure label={t('admin.storage.cache')} value={bytes(stats.cache_bytes)} hint={t('admin.storage.cacheHint')} />
          <Figure label={t('admin.storage.free')} value={bytes(stats.disk_free_bytes)} hint={t('admin.storage.freeHint')} />
        </ul>
        <div className={cx(boxClasses, 'p-4')}>
          <Meter value={share} label={t('admin.storage.share')} detail={t('admin.storage.shareDetail', { stored: bytes(stats.blob_bytes), room: bytes(room) })} />
        </div>
      </Section>

      <Section id="admin-storage-consumers" title={t('admin.storage.consumersTitle')} lead={t('admin.storage.consumersLead')}>
        {stats.top_environments.length === 0 ? (
          <EmptyState compact icon={<HardDrive aria-hidden />} title={t('admin.storage.noConsumers')} />
        ) : (
          <Table label={t('admin.storage.consumersTitle')} columns={columns} rows={stats.top_environments} rowKey={(row) => row.environment.id} />
        )}
      </Section>
    </div>
  );
}
