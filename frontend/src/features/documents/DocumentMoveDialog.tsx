/** A document or complete folder keeps its identity inside its org and moves out through one native act. */
import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { ApiError, client } from '@/api/client';
import { documentDirectoryQuery, documentTreeEntryQuery } from '@/api/documentTree';
import { documentMoveOpening, executeDocumentMove, previewDocumentMoveDeletion, type DocumentMoveBody } from '@/api/documentMove';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import type { CopyDestinationJudgement } from '@/api/copyDestinations';
import { CopyPlacePicker } from '@/features/duplicate/CopyPlacePicker';
import { NamedChip } from '@/features/people/NamedChip';
import { NamePicker } from '@/features/search/NamePicker';
import type { PickedPlace } from '@/features/settings/PlacePicker';
import type { DocumentDirectory, DocumentDirectoryContext, DocumentTreeListed, NameHit } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
import { newActKey } from '@/lib/requests';
import { formatNumber } from '@/lib/format';
import { WAY_SEPARATOR } from '@/features/finder/destinations';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { StateLine } from '@/ui/StateLine';
import { Table } from '@/ui/Table';
import { toast } from '@/ui/Toast';
import { deletionPaths } from './treeLocation';

export function DocumentMoveDialog({ row, directory, onClose, onDone }: { row: DocumentTreeListed; directory?: DocumentDirectoryContext | undefined; onClose: () => void; onDone?: (() => void) | undefined }) {
  const { t, i18n } = useTranslation();
  const cache = useQueryClient(), navigate = useNavigate(), key = useActKey();
  const [reader] = useState(libraryReader);
  const current = useStore(client.session, () => sameLibraryReader(reader));
  const [previewId] = useState(newActKey);
  const [picked, setPicked] = useState<PickedPlace | null>(() => directory === undefined ? null : { id: directory.node.id, kind: directory.node.kind, color: null, folder: false, rootOrgId: row.entry.root, rootName: directory.path.find((node) => node.id === row.entry.root && !node.redacted)?.name ?? t('access.place.hidden'), path: directory.path.map((node) => node.redacted ? t('ui.nameUnknown') : node.name ?? t('ui.nameUnknown')).join(WAY_SEPARATOR) });
  const [folder, setFolder] = useState<Pick<NameHit, 'id' | 'name'> | null>(() => directory?.folder === null || directory?.folder === undefined ? null : { id: directory.folder.entry.item.id, name: directory.folder.entry.title });
  const [name, setName] = useState(row.entry.basename);
  const [standing, setStanding] = useState<CopyDestinationJudgement | null>(null);
  const [review, setReview] = useState(0);
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const at: DocumentDirectory | null = folder !== null ? { kind: 'container', id: folder.id } : picked === null ? null : { kind: 'node_docs', id: picked.id };
  const folderRead = useQuery({ ...documentTreeEntryQuery({ kind: 'container', id: folder?.id ?? '' }, cache, false, JSON.stringify(reader)), enabled: current && folder !== null });
  const rootRead = useQuery({ ...documentDirectoryQuery({ node: picked?.id ?? '', at: { kind: 'node_docs', id: picked?.id ?? '' } }, cache), enabled: current && picked !== null && folder === null });
  const context = (folder === null ? rootRead.data?.directory : folderRead.data?.directory) ?? undefined;
  const targetRead = folder === null ? rootRead : folderRead;
  const folderMatches = folder === null || context?.at.kind === 'container' && context.at.id === folder.id && context.folder?.entry.root === picked?.rootOrgId;
  const place: PickedPlace | null = folder !== null && context !== undefined && folderMatches && picked !== null
    ? { ...picked, id: context.node.id, kind: context.node.kind, path: context.path.map((node) => node.redacted ? t('ui.nameUnknown') : node.name ?? t('ui.nameUnknown')).join(WAY_SEPARATOR) } : picked;
  const across = standing?.cross_root === true;
  const destinationReady = current && at !== null && context?.at.kind === at.kind && context.at.id === at.id && context.can_create && folderMatches && !targetRead.isError && !targetRead.isFetching;
  const nativeReady = standing !== null && standing.outcome.state !== 'blocked';
  const preview = useQuery({ queryKey: ['document-move-delete-preview', reader, previewId, row.entry.item, at, review],
    queryFn: () => previewDocumentMoveDeletion([row.entry.item], previewId), enabled: destinationReady && nativeReady && across, retry: false, staleTime: Infinity, gcTime: 0, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const plan = preview.data === undefined || 'projection' in preview.data ? undefined : preview.data;
  const confirmation = preview.data === undefined ? null : 'projection' in preview.data ? preview.data.deletion_plan : preview.data.stamp;
  const sameParent = at?.kind === row.entry.parent.kind && at.id === row.entry.parent.id;
  const insideSource = row.entry.item.kind === 'container' && (at?.kind === 'container' && at.id === row.entry.item.id || context?.ancestors.some((ancestor) => ancestor.id === row.entry.item.id) === true);
  const refusal = insideSource ? t('duplicate.folderInsideSource') : standing?.outcome.state === 'blocked' ? t(`duplicate.causes.${standing.outcome.cause}`) : !folderMatches || targetRead.isSuccess && context === undefined ? t('documentFolders.unavailable') : !destinationReady || !nativeReady ? t('duplicate.checking')
    : across && (!preview.isSuccess || preview.isFetching) ? t(preview.isError ? 'duplicate.movePreviewFailed' : 'duplicate.checking') : !across && sameParent && (name.trim() === '' || name.trim() === row.entry.basename) ? t('duplicate.samePlace') : false;
  const write = useMutation({ mutationFn: (body: DocumentMoveBody) => {
    if (!sameLibraryReader(reader)) throw new DOMException('The document reader changed.', 'AbortError');
    return executeDocumentMove(body);
  }, onSuccess: (answer, body) => {
    if (!mounted.current || !sameLibraryReader(reader)) return;
    toast({ tone: 'success', title: t('documentFolders.done') }); onDone?.(); onClose();
    if (body.command === 'move_out' && body.to !== null) void navigate({ href: documentMoveOpening(answer, row.entry.item, body.to) });
  } });
  const content = { command: across ? 'move_out' as const : 'move' as const, items: [row.entry.item], to: at, name: name.trim() === row.entry.basename ? null : name.trim() || null, version: null, deletion_plan: across ? confirmation : null };
  const retry = write.isError && JSON.stringify({ to: write.variables.to, name: write.variables.name }) === JSON.stringify({ to: at, name: content.name });
  const destructive = retry ? write.variables.command === 'move_out' : across;
  const reviewAgain = () => { write.reset(); key.reset(); setReview((before) => before + 1); };
  const submit = () => {
    if (!current || write.isPending || at === null) return;
    if (retry) { write.mutate(write.variables); return; }
    if (refusal !== false) return;
    const snapshot = structuredClone(content);
    write.mutate({ ...snapshot, client_id: key(snapshot) });
  };
  const paths = deletionPaths(plan?.stamp.entries ?? []);
  return <Dialog open title={t('documentFolders.actions.move')} dismissible={!write.isPending} onOpenChange={(open) => { if (!open && !write.isPending) onClose(); }} footer={<>
    <span className="me-auto min-w-0 text-sm text-text-muted">{current && at !== null ? targetRead.isError ? t('duplicate.movePreviewFailed') : context !== undefined && !context.can_create ? t('people.destinationNeedsCreate') : refusal : null}</span>
    <Button variant="ghost" disabled={write.isPending} onClick={onClose}>{t('common.cancel')}</Button>
    <Button variant={destructive ? 'danger' : 'primary'} disabled={!current || at === null || !retry && refusal !== false} pending={write.isPending} onClick={submit}>{destructive ? t('duplicate.moveAcross', { org: place?.rootName ?? t('access.place.hidden') }) : t('duplicate.moveConfirm')}</Button>
  </>}>
    {!current ? <StateLine compact message={t('documentFolders.unavailable')} /> : <div className="flex min-w-0 flex-col gap-4">
      <CopyPlacePicker label={t('duplicate.destination')} includeThreads subject={{ kind: row.entry.item.kind === 'container' ? 'document_container' : 'document', id: row.entry.item.id }} sourceName={row.entry.title} intent="move" value={place} onStanding={setStanding} disabled={write.isPending} onChange={(next) => { setPicked(next); setFolder(null); write.reset(); }} />
      {picked !== null && <fieldset className="min-w-0" disabled={write.isPending}><NamePicker pickerKey="documents.Move.folder" label={t('duplicate.folderOptional')} chosen={folder?.name ?? null} orgId={picked.rootOrgId} kinds={['document_container']} placeholder={t('documentFolders.findDestination')} nothing={t('documents.move.none')} onChoose={(hit) => { setFolder(hit); write.reset(); }} /></fieldset>}
      {targetRead.isError && at !== null && <ErrorState compact error={targetRead.error} onRetry={() => void targetRead.refetch()} />}
      {at !== null && targetRead.isPending && <SkeletonRows read={targetRead} rows={1} />}
      <Input label={t('documentFolders.name')} value={name} disabled={write.isPending} onChange={(event) => { setName(event.target.value); write.reset(); }} />
      {across && (preview.isError ? <ErrorState compact error={preview.error} onRetry={reviewAgain} /> : preview.data === undefined ? <SkeletonRows read={preview} rows={2} /> : plan === undefined ? <StateLine compact message={t('duplicate.deletePlanReady')} /> : <>
        <Table label={t('duplicate.deletePlan')} rows={plan.stamp.entries} rowKey={(entry) => `${entry.item.kind}:${entry.item.id}`} columns={[
          { id: 'name', header: t('documentFolders.name'), cell: (entry) => entry.item.kind === 'document' ? <NamedChip kind="doc" id={entry.item.id} name={paths.get(`document:${entry.item.id}`) ?? entry.basename} path={`/doc/${encodeURIComponent(entry.item.id)}`} /> : <Link to="/document-folder/$folderId" params={{ folderId: entry.item.id }} className="break-all text-accent hover:underline">{paths.get(`container:${entry.item.id}`) ?? entry.basename}</Link> },
          { id: 'files', header: t('duplicate.files'), cell: (entry) => formatNumber(entry.files, i18n.language) },
          { id: 'versions', header: t('duplicate.versions'), cell: (entry) => formatNumber(entry.versions, i18n.language) },
        ]} />
        <span className="text-sm text-text-muted">{t('duplicate.restorableUntil')} <RelativeTime at={plan.restore_until} /></span>
      </>)}
      {write.isError && <ErrorAlert error={write.error} action={write.error instanceof ApiError && write.error.is('conflict.stale_revision') ? <Button onClick={reviewAgain}>{t('duplicate.reviewAgain')}</Button> : undefined} />}
    </div>}
  </Dialog>;
}
