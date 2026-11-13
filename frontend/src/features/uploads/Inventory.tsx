/** Upload inventory facts stay native; completed administrative bytes use their custody id. */
import { useInfiniteQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { deleteOwnUpload, ownUploadsQuery, reachKeys, refreshReach, uploadInventoryContentPath } from '@/api/adminReach';
import type { UploadAttachment, UploadInventoryItem } from '@/generated/api-types';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { FileOverlay } from '@/features/files/FileOverlay';
import type { FileSource } from '@/features/files/source';
import { NamedChip } from '@/features/people/NamedChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { resourceSubjectOf } from '@/features/resource-facts/facts';
import { Popover } from '@/ui/Popover';
import { MachineLink } from '@/features/resources/machines/MachineLink';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { AsAdminMark } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusChip } from '@/ui/StateChip';
import { Table, type Column } from '@/ui/Table';
import { Tooltip } from '@/ui/Tooltip';

export function UploadState({ row }: { row: UploadInventoryItem }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  return <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-sm text-text-muted"><StatusChip tone={row.state === 'expired' ? 'warning' : row.state === 'complete' ? 'success' : 'neutral'}>{t(`adminArtifacts.uploadStates.${row.state}`)}</StatusChip>
    <span className="tabular-nums">{row.state === 'complete' ? formatBytes(row.size, locale) : `${formatBytes(row.received, locale)} / ${formatBytes(row.size, locale)}`}</span>
    {row.expires_at !== null && <RelativeTime at={row.expires_at} />}
  </span>;
}
/** This URL never substitutes a blob hash for administrative custody. */
export function inventoryFile(row: UploadInventoryItem, name: string, admin: boolean): FileSource | null {
  const url = uploadInventoryContentPath(row, name, admin);
  return url === null ? null : { url, name, size: row.size, mediaType: row.media_type ?? undefined, version: row.updated_at,
    ...(admin ? { adminOnly: true } : {}), archiveUrl: `${url}&archive=true` };
}
export function UploadOpen({ row, admin = false }: { row: UploadInventoryItem; admin?: boolean }) {
  const { t } = useTranslation();
  const [opened, setOpened] = useState(false);
  const source = inventoryFile(row, row.name ?? t('storage.kinds.upload'), admin);
  return source === null ? null : <><Button size="sm" variant="ghost" onClick={() => setOpened(true)}>{t('adminArtifacts.open')}</Button>{opened && <FileOverlay source={source} onClose={() => setOpened(false)} />}</>;
}
/** Only identities/names actually disclosed by their ordinary cards appear; no labels are guessed. */
export function UploadAttachments({ row, renderArtifact }: { row: UploadInventoryItem; renderArtifact?: (reference: UploadAttachment) => ReactNode }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  if (row.attachment_count === 0) return <span className="text-sm text-text-muted">{t('adminArtifacts.unattached')}</span>;
  const label = t('adminArtifacts.attached', { number: formatNumber(row.attachment_count, locale) });
  const more = Math.max(0, row.attachment_count - row.attached_to.length);
  return <Popover label={label} trigger={<Button size="sm" variant="ghost">{label}</Button>}>
    <ul className="flex min-w-0 flex-col gap-2">{row.attached_to.map((reference) => {
      let content: ReactNode;
      if (reference.kind === 'thread' || reference.kind === 'document' || reference.kind === 'chat') {
        const kind = namedReferenceKinds[reference.kind];
        content = <NamedChip kind={kind} id={reference.id} path={`/${kind}/${encodeURIComponent(reference.id)}`} original={t(`finder.kinds.${reference.kind}`)} />;
      } else if (reference.kind === 'environment') {
        const subject = resourceSubjectOf({ kind: reference.kind, id: reference.id }, t('finder.kinds.environment'));
        content = subject === null ? t('finder.kinds.environment') : <ResourceChip subject={subject} hint={false} />;
      } else if (reference.kind === 'snapshot' || reference.kind === 'export' || reference.kind === 'import') {
        content = <span className="flex min-w-0 items-center justify-between gap-2"><span>{t(`storage.kinds.${reference.kind}`)}</span>{renderArtifact?.(reference)}</span>;
      } else content = t('adminArtifacts.reference');
      return <li key={`${reference.kind}:${reference.id}`} className="min-w-0">{content}</li>;
    })}{more > 0 && <li className="text-sm text-text-muted">{t('adminArtifacts.moreReferences', { number: formatNumber(more, locale) })}</li>}</ul>
  </Popover>;
}

export function OwnUploads() {
  const reader = useStore(client.session, (state) => JSON.stringify([state.userId, state.readerVersion]));
  return <UploadsReader key={reader} />;
}
const namedReferenceKinds = { thread: 'thread', document: 'doc', chat: 'chat' } as const;

function UploadsReader() {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const list = useInfiniteQuery(ownUploadsQuery());
  const rows = list.data?.pages.flatMap((page) => page.items) ?? [];
  const [ending, setEnding] = useState<UploadInventoryItem | null>(null);
  const remove = useMutation({
    mutationFn: ({ row }: { row: UploadInventoryItem; reader: ReturnType<typeof libraryReader> }) => deleteOwnUpload(row),
    onSuccess: (_answer, asked) => { if (sameLibraryReader(asked.reader)) { setEnding(null); void refreshReach(cache, reachKeys.uploads); } },
  });
  const columns: Column<UploadInventoryItem>[] = [
    { id: 'name', header: t('storage.name'), grow: true, cell: (row, shown) => {
      const showCreated = shown.has('created');
      return <div className="flex min-w-0 flex-col items-start gap-1"><Tooltip content={row.name ?? t('storage.kinds.upload')}><span tabIndex={0} className="max-w-full truncate font-medium">{row.name ?? t('storage.kinds.upload')}</span></Tooltip>
      <UploadState row={row} />{row.machine !== null && <MachineLink machine={row.machine} />}
      {!showCreated && <RelativeTime at={row.created_at} className="text-sm text-text-muted" />}{row.admin_access !== null && <AsAdminMark of={row.admin_access.place} />}
    </div>; } },
    { id: 'created', header: t('adminCredentials.created'), from: 'sm', cell: (row) => <RelativeTime at={row.created_at} /> },
    { id: 'updated', header: t('adminArtifacts.updated'), from: 'md', cell: (row) => <RelativeTime at={row.updated_at} /> },
    { id: 'acts', header: t('admin.users.actions'), hideHeader: true, align: 'end', cell: (row) => <span className="flex justify-end gap-2"><UploadOpen row={row} />
      <Button size="sm" variant="danger-ghost" blocked={!row.can_delete || row.kind === 'machine' || row.kind === 'session' && row.session_id === null || row.kind === 'completed' && row.blob === null}
        reason={t('adminArtifacts.cannotDeleteUpload')} disabled={remove.isPending} onClick={() => { remove.reset(); setEnding(row); }}>{t('adminArtifacts.delete')}</Button></span> },
  ];
  return <div className="flex min-w-0 flex-col gap-4"><div className="flex justify-end"><Button size="sm" disabled={list.isFetching} onClick={() => void list.refetch()}>{t('operationKeys.activity.refresh')}</Button></div>
    {list.isPending ? <SkeletonRows read={list} rows={4} /> : list.isError && !list.isFetchNextPageError ? <ErrorState error={list.error} onRetry={() => void list.refetch()} />
      : rows.length === 0 ? <EmptyState compact title={t('adminArtifacts.noUploads')} /> : <Table label={t('adminArtifacts.uploads')} rows={rows} rowKey={(row) => row.id} columns={columns} />}
    <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
    <ConfirmDialog open={ending !== null} onOpenChange={(open) => { if (!open && !remove.isPending) setEnding(null); }} title={t('adminArtifacts.deleteUpload', { name: ending?.name ?? t('storage.kinds.upload') })}
      confirmLabel={t('adminArtifacts.delete')} danger pending={remove.isPending} error={remove.error ?? undefined} onConfirm={() => { if (ending !== null) remove.mutate({ row: ending, reader: libraryReader() }); }}>
      {t(ending?.kind === 'session' ? 'adminArtifacts.cancelUploadBody' : 'adminArtifacts.releaseUploadBody')}
    </ConfirmDialog>
  </div>;
}
