/** The verified manifest's actual restore inventory, with unknown locations and uninspected copies kept unknown. */
import { useQuery } from '@tanstack/react-query';
import { FileText } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { backupRestoreListQuery } from '@/api/admin';
import { Facts } from '@/features/resources/Facts';
import type { BackupInventoryGroup, BackupInventoryLocation, DatabaseBackupRecord } from '@/generated/api-types';
import type { TranslationKey } from '@/i18n';
import { Button } from '@/ui/Button';
import { CopyButton } from '@/ui/CopyButton';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';

const CATEGORIES: Readonly<Record<string, TranslationKey | undefined>> = {
  database: 'backupRestore.categories.database', blobs: 'backupRestore.categories.blobs',
  configuration: 'backupRestore.categories.configuration', keys: 'backupRestore.categories.keys',
  credentials: 'backupRestore.categories.credentials', workspaces: 'backupRestore.categories.workspaces',
  homes: 'backupRestore.categories.homes', provider_states: 'backupRestore.categories.provider_states',
  browser_profiles: 'backupRestore.categories.browser_profiles', mounts: 'backupRestore.categories.mounts',
  private_overlays: 'backupRestore.categories.private_overlays', connected_directories: 'backupRestore.categories.connected_directories',
  edge_volumes: 'backupRestore.categories.edge_volumes',
};

function Location({ location }: { location: BackupInventoryLocation }) {
  const { t } = useTranslation();
  return <li className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3 text-sm">
    <Facts rows={[
      { name: t('backupRestore.location'), value: location.location_known && location.location !== null ? <code className="wrap-anywhere">{location.location}</code> : t('backupRestore.unknownLocation') },
      { name: t('backupRestore.copy'), value: t(location.copy_available === null ? 'backupRestore.uninspectedCopy' : location.copy_available ? 'backupRestore.copyFound' : 'backupRestore.noCopy') },
      ...([['object', location.object_id], ['host', location.host_id], ['machine', location.machine_id], ['runtime', location.runtime_id], ['driver', location.driver], ['storage', location.storage], ['mount', location.mount]] as const).flatMap(([kind, value]) => value === null ? [] : [{ name: t(`backupRestore.fields.${kind}`), value: <code className="wrap-anywhere">{value}</code> }]),
    ]} />
  </li>;
}

function Group({ group }: { group: BackupInventoryGroup }) {
  const { t } = useTranslation();
  const key = CATEGORIES[group.category];
  return <li className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-3">
    <h4 className="font-medium text-text">{key === undefined ? group.category : t(key)}</h4>
    <p className="text-sm text-text-muted">{group.count === null ? t('backupRestore.countUnknown') : t('backupRestore.count', { count: group.count })}</p>
    {group.environments_requiring_inspection !== null && <p className="text-sm text-text-muted">{t('backupRestore.inspect', { count: group.environments_requiring_inspection })}</p>}
    <p className="text-sm wrap-anywhere text-text-muted">{group.reason}</p>
    {group.locations.length === 0 ? <EmptyState compact title={t('backupRestore.noLocations')} /> : <ul className="flex flex-col gap-2">{group.locations.map((location, index) => <Location key={index} location={location} />)}</ul>}
  </li>;
}

export function BackupRestoreList({ expected }: { expected: DatabaseBackupRecord | null }) {
  const { t } = useTranslation();
  const query = useQuery(backupRestoreListQuery);
  if (query.isPending) return <SkeletonRows rows={5} />;
  if (query.isError) return <ErrorState compact error={query.error} onRetry={() => void query.refetch()} retrying={query.isRefetching} />;
  const list = query.data;
  if (list === null) return <EmptyState compact icon={<FileText aria-hidden />} title={t('backupRestore.none')} description={t('backupRestore.noneAbout')} />;
  if (expected !== null && (list.backup.hash !== expected.hash || list.backup.dump_name !== expected.dump_name || list.backup.completed_at !== expected.completed_at)) return <InlineAlert tone="warning">{t('backupRestore.changed')}</InlineAlert>;
  return <div className="flex min-w-0 flex-col gap-4">
    {query.isFetching && <InlineAlert tone="info">{t('backupRestore.checking')}</InlineAlert>}
    <InlineAlert tone="info">{t('backupRestore.separate')}</InlineAlert>
    <Facts rows={[
      { name: t('backupRestore.dump'), value: <code className="wrap-anywhere">{list.backup.dump_name}</code> },
      { name: t('backupRestore.completed'), value: <RelativeTime at={list.backup.completed_at} /> },
      { name: t('backupRestore.manifest'), value: <span className="flex min-w-0 items-start gap-2"><code className="min-w-0 flex-1 wrap-anywhere">{list.manifest_name}</code><CopyButton value={list.manifest_name} label={t('installationRecords.copyManifest')} /></span> },
      { name: t('backupRestore.hash'), value: <code className="wrap-anywhere">{list.manifest_hash}</code> },
    ]} />
    <section aria-label={t('backupRestore.included')} className="flex flex-col gap-2"><h3 className="font-medium text-text">{t('backupRestore.included')}</h3><ul className="flex flex-col gap-3">{list.included.map((group) => <Group key={group.category} group={group} />)}</ul></section>
    <section aria-label={t('backupRestore.excluded')} className="flex flex-col gap-2"><h3 className="font-medium text-text">{t('backupRestore.excluded')}</h3><ul className="flex flex-col gap-3">{list.excluded.map((group) => <Group key={group.category} group={group} />)}</ul></section>
    <Button size="sm" variant="ghost" pending={query.isFetching} onClick={() => void query.refetch()}>{t('actions.refresh.label')}</Button>
  </div>;
}
