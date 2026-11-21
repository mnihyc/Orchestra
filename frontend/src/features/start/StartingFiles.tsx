/** Starting files use the same authorized names, file selection and recorded states as the source screens. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { environmentQuery } from '@/api/environments';
import { ApiError } from '@/api/client';
import { snapshotQuery, threadSnapshotsQuery } from '@/api/source';
import { FilesChoice } from '@/features/change/Files';
import { selectionOf, type FileChoice } from '@/features/change/choices';
import { NamedChip } from '@/features/people/NamedChip';
import { ResourceChip } from '@/features/resource-facts/ResourceFacts';
import { NamePicker } from '@/features/search/NamePicker';
import { restorable, unrecordedReason } from '@/features/source/words';
import type { NameHit, NewEnvironmentFiles, Snapshot } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { ResourcePicker } from '@/ui/ResourcePicker';
import { SkeletonRows } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

function SnapshotName({ id }: { id: string }) {
  const read = useQuery(snapshotQuery(id));
  if (read.isError) return <ErrorAlert error={read.error} />;
  if (read.data === undefined) return <SkeletonRows rows={1} />;
  return <span className="min-w-0 truncate" title={read.data.label}>{read.data.label}</span>;
}

function EnvironmentName({ id }: { id: string }) {
  const read = useQuery(environmentQuery(id));
  if (read.isError) return <ErrorAlert error={read.error} />;
  if (read.data === undefined) return <SkeletonRows rows={1} />;
  return <ResourceChip subject={{ kind: 'environment', environment: read.data }} hint={false} />;
}

export function StartingFiles({ value, onChange, disabled = false }: { value?: NewEnvironmentFiles | null; onChange: (value: NewEnvironmentFiles | undefined) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const { scope } = useCollectionSearch('starting-files');
  const [open, setOpen] = useState(false);
  return <div className="flex min-w-0 flex-col gap-2">
    <div className="flex min-w-0 flex-wrap items-center gap-2">
      <Button size="sm" disabled={disabled} onClick={() => setOpen(true)}>{t('start.where.filesFrom')}</Button>
      {value !== undefined && value !== null && <>
        {'from_thread' in value ? <NamedChip kind="thread" id={value.from_thread} path={`/thread/${value.from_thread}`} /> : 'from_environment' in value ? <EnvironmentName id={value.from_environment} /> : <SnapshotName id={value.snapshot_id} />}
        <Button size="sm" variant="ghost" disabled={disabled} onClick={() => onChange(undefined)}>{t('start.where.clearFiles')}</Button>
      </>}
    </div>
    {open && <StartingFilesDialog key={scope} onClose={() => setOpen(false)} onChoose={(files) => { onChange(files); setOpen(false); }} />}
  </div>;
}

function StartingFilesDialog({ onClose, onChoose }: { onClose: () => void; onChoose: (files: NewEnvironmentFiles) => void }) {
  const { t } = useTranslation();
  const [thread, setThread] = useState<NameHit | null>(null);
  const [mode, setMode] = useState<'current' | 'snapshot'>('current');
  const [files, setFiles] = useState<FileChoice>({ mode: 'all' });
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const snapshotSearch = useCollectionSearch('starting-snapshots:' + (thread?.id ?? ''));
  const snapshots = useInfiniteQuery({ ...threadSnapshotsQuery(thread?.id ?? '', snapshotSearch.q), enabled: thread !== null && mode === 'snapshot' });
  const recorded = snapshots.data?.pages.flatMap((page) => page.items) ?? [];
  const pickedSnapshot = recorded.find((row) => row.id === snapshot?.id) ?? snapshot ?? undefined;
  const refused = snapshots.error instanceof ApiError && (snapshots.error.status === 403 || snapshots.error.status === 404);
  const ready = thread !== null && (mode === 'current' || !refused && pickedSnapshot !== undefined && restorable(pickedSnapshot));
  return <Dialog open title={t('start.where.filesFrom')} size="md" onOpenChange={(open) => { if (!open) onClose(); }} footer={<>
    <Button onClick={onClose}>{t('common.cancel')}</Button>
    <Button variant="primary" disabled={!ready} reason={!ready ? t(mode === 'current' || thread === null ? 'start.where.chooseFileThread' : 'start.where.chooseSnapshot') : undefined}
      onClick={() => {
        if (thread === null) return;
        if (mode === 'current') onChoose({ from_thread: thread.id, selection: selectionOf(files) });
        else if (pickedSnapshot !== undefined && restorable(pickedSnapshot)) onChoose({ snapshot_id: pickedSnapshot.id });
      }}>{t('start.where.useFiles')}</Button>
  </>}>
    <div className="flex min-w-0 flex-col gap-3">
      <NamePicker pickerKey="starting-file-thread" label={t('start.where.fileThread')} kinds={['thread']} chosen={thread?.name ?? null}
        placeholder={t('start.where.findThread')} nothing={t('start.where.noThreads')} onChoose={(next) => { setThread(next); setSnapshot(null); }} />
      {thread !== null && <>
        <NamedChip kind="thread" id={thread.id} path={`/thread/${thread.id}`} name={thread.name} />
        <SegmentedControl<'current' | 'snapshot'> label={t('start.where.fileState')} value={mode} onValueChange={setMode} segments={[
          { value: 'current', name: t('start.where.currentFiles') }, { value: 'snapshot', name: t('start.where.recordedFiles') },
        ]} />
        {mode === 'current' ? <FilesChoice value={files} onChange={setFiles} /> : <>
          <CollectionSearch name={t('start.where.snapshot')} {...snapshotSearch} />
          {snapshots.isPending ? <SkeletonRows rows={2} /> : snapshots.isError && recorded.length === 0 ? <ErrorState compact error={snapshots.error} onRetry={() => void snapshots.refetch()} /> : recorded.length === 0 ? <p className="text-sm text-text-muted">{t('start.where.noSnapshots')}</p> : <ResourcePicker pickerKey="start.recorded-source" label={t('start.where.snapshot')} value={snapshot?.id ?? ''} valueLabel={pickedSnapshot?.label} placeholder={t('start.where.chooseSnapshot')} onValueChange={(id) => { const selected = recorded.find((row) => row.id === id); if (selected !== undefined) { setSnapshot(selected); } }}
            options={recorded.map((row) => ({ value: row.id, label: row.label, disabled: !restorable(row), description: <span className="flex flex-wrap items-center gap-2">
              <RelativeTime at={row.created_at} /><ByteSize bytes={row.total_bytes} />{row.created_by !== null && <UserChip user={row.created_by} />}
              {!restorable(row) && <span>{unrecordedReason(t, row)}</span>}
            </span> }))} />}
          <Pagination hasMore={snapshots.hasNextPage} loading={snapshots.isFetchingNextPage} error={snapshots.isFetchNextPageError ? snapshots.error : null} onMore={() => void snapshots.fetchNextPage()} />
        </>}
      </>}
    </div>
  </Dialog>;
}
