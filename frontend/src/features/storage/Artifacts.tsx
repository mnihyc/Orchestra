/** Storage opens the existing custody readers, never the opaque accounting key. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { meQuery } from '@/api/auth';
import { artifactSnapshotQuery, exportManifestQuery } from '@/api/adminReach';
import { ADMIN_QUERY, queryAuthorityAllows } from '@/api/queryAuthority';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { downloadExport } from '@/api/exports';
import { useAdminMode } from '@/features/access/AdminMode';
import { ImportReport } from '@/features/people/ImportReport';
import { saveFile } from '@/features/export/save';
import { SnapshotDownloadButton } from '@/features/resources/environments/WorkspaceExport';
import { UploadAttachments, UploadOpen, UploadState } from '@/features/uploads/Inventory';
import type { KeptRecord, StorageArtifact } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber } from '@/lib/format';
import { AsAdminMark } from '@/ui/AsAdminMark';
import { Button } from '@/ui/Button';
import { CodeBlock } from '@/ui/CodeBlock';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { UserChip } from '@/ui/UserChip';

function SnapshotFacts({ id, admin }: { id: string; admin: boolean }) {
  const { t } = useTranslation(); const { locale } = useFormatContext();
  const snapshot = useQuery(artifactSnapshotQuery(id, admin));
  if (snapshot.isPending) return <SkeletonRows read={snapshot} rows={3} />;
  if (snapshot.isError) return <ErrorState error={snapshot.error} onRetry={() => void snapshot.refetch()} />;
  const row = snapshot.data;
  return <div className="flex min-w-0 flex-col items-start gap-3"><div className="flex flex-wrap items-center gap-2">
    {row.created_by !== null && <UserChip user={row.created_by} />}<RelativeTime at={row.created_at} />
    {row.admin_access !== undefined && row.admin_access !== null && <AsAdminMark of={row.admin_access.place} />}
  </div><StateLine title={t(`adminArtifacts.snapshotStates.${row.state}`)} message={`${formatNumber(row.file_count, locale)} · ${formatBytes(row.total_bytes, locale)}`} />
    {row.missing_count > 0 && <StateLine tone="warning" title={t('adminArtifacts.missingFiles', { number: formatNumber(row.missing_count, locale) })} />}
    {(row.state === 'complete' || row.state === 'partial') && <SnapshotDownloadButton id={row.id} name={row.label || t('storage.kinds.snapshot')} admin={admin} />}
  </div>;
}
function ExportManifest({ id, admin }: { id: string; admin: boolean }) {
  const { t } = useTranslation(); const cache = useQueryClient();
  const info = useQuery(exportManifestQuery(id, admin));
  const download = useMutation({ mutationFn: (_asked: { reader: ReturnType<typeof libraryReader> }) => downloadExport(id, () => undefined),
    onSuccess: (file, asked) => { if (sameLibraryReader(asked.reader) && (!admin || queryAuthorityAllows(cache, ADMIN_QUERY.meta))) saveFile(file.blob, file.name); } });
  if (info.isPending) return <SkeletonRows read={info} rows={3} />;
  if (info.isError) return <ErrorState error={info.error} onRetry={() => void info.refetch()} />;
  const row = info.data;
  return <div className="flex min-w-0 flex-col gap-3">
    {row.admin_access !== undefined && row.admin_access !== null && <AsAdminMark of={row.admin_access.place} />}
    {row.manifest === undefined || row.manifest === null ? <StateLine title={t('adminArtifacts.noManifest')} /> : <CodeBlock code={JSON.stringify(row.manifest, null, 2)} lang="json" />}
    {row.state === 'ready' && <div><Button size="sm" pending={download.isPending} onClick={() => download.mutate({ reader: libraryReader() })}>{t('temporary.download')}</Button></div>}
    {download.error !== null && <ErrorAlert error={download.error} />}
  </div>;
}
export function ArtifactOpen({ artifact, admin }: { artifact: Pick<StorageArtifact, 'id' | 'kind'>; admin: boolean }) {
  const { t } = useTranslation(); const [open, setOpen] = useState(false);
  return <><Button size="sm" variant="ghost" onClick={() => setOpen(true)}>{t('adminArtifacts.open')}</Button>{open && <Dialog key={`${artifact.kind}:${artifact.id}:${admin}`} open size="lg"
    title={t(`storage.kinds.${artifact.kind}`)} onOpenChange={setOpen} footer={<Button onClick={() => setOpen(false)}>{t('common.close')}</Button>}>
    {artifact.kind === 'import' ? <ImportReport id={artifact.id} inspectOnly={admin} /> : artifact.kind === 'snapshot' ? <SnapshotFacts id={artifact.id} admin={admin} /> : <ExportManifest id={artifact.id} admin={admin} />}
  </Dialog>}</>;
}
export function StorageOpening({ row }: { row: KeptRecord }) {
  const me = useQuery(meQuery), mode = useAdminMode();
  const admin = mode.data?.as_admin === true && me.data?.is_system_admin !== false;
  return <div className="flex min-w-0 flex-col items-start gap-1">{row.upload !== undefined && row.upload !== null && <>
    <UploadState row={row.upload} /><UploadAttachments row={row.upload} renderArtifact={(reference) => reference.kind === 'snapshot' || reference.kind === 'export' || reference.kind === 'import' ? <ArtifactOpen artifact={{ id: reference.id, kind: reference.kind }} admin={admin} /> : null} />{admin && row.upload.can_open && <UploadOpen row={row.upload} admin />}
  </>}{row.artifact !== undefined && row.artifact !== null && <ArtifactOpen artifact={row.artifact} admin={admin} />}</div>;
}
