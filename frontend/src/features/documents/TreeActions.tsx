/** One document operation; only deletion first reads a complete affected-tree preview. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { documentDirectoryQuery, documentTreeEntryQuery, organizeDocuments, previewDocumentDeletion } from '@/api/documentTree';
import { documentAncestors } from '@/features/documents/treeLocation';
import { WAY_SEPARATOR } from '@/features/finder/destinations';
import { NamePicker } from '@/features/search/NamePicker';
import type { DocumentDirectory, DocumentDirectoryContext, DocumentTreeEntry, DocumentTreeListed, DocumentTreeOperationBody } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { useActKey } from '@/lib/forms';
import { formatExact } from '@/lib/time';
import { newActKey } from '@/lib/requests';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

export type TreeAction = 'create_folder' | 'copy' | 'move' | 'rename' | 'delete';
interface Target { at: DocumentDirectory; label: string }

/** The destination's own native metadata decides Create, including archive and docs policy. */
function DestinationStanding({ target, onRead }: { target: Target; onRead: (value: DocumentDirectoryContext | null) => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const folder = useQuery({ ...documentTreeEntryQuery({ kind: 'container', id: target.at.id }, cache), enabled: target.at.kind === 'container' });
  const root = useQuery({ ...documentDirectoryQuery({ node: target.at.id, at: target.at }, cache), enabled: target.at.kind === 'node_docs' });
  const query = target.at.kind === 'container' ? folder : root;
  const value = query.data?.directory;
  useEffect(() => { onRead(query.isSuccess && (value !== undefined && value !== null) ? value : null); }, [onRead, query.isSuccess, value]);
  if (query.isError) return <ErrorAlert error={query.error} action={<Button onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />;
  if (query.isPending) return <SkeletonRows rows={1} />;
  if (value === undefined || value === null) return null;
  const names = [...value.path.map((step) => step.redacted ? t('ui.nameUnknown') : step.name ?? t('ui.nameUnknown')), t('documents.list.title'),
    ...documentAncestors(value.folder?.entry.parent ?? value.at, value.ancestors).map((step) => step.title ?? t('ui.nameUnknown')),
    ...(value.folder === null ? [] : [value.folder.entry.title])];
  return <p className="break-words text-sm text-text-muted">{names.join(WAY_SEPARATOR)}</p>;
}

/** All preview entries are authorized. Resolve their selected-tree paths without hidden ancestors. */
export function deletionPaths(entries: readonly DocumentTreeEntry[]): Map<string, string> {
  const folders = new Map(entries.filter((entry) => entry.item.kind === 'container').map((entry) => [entry.item.id, entry]));
  const result = new Map<string, string>();
  for (const entry of entries) {
    const parts = [entry.basename];
    const seen = new Set([`${entry.item.kind}:${entry.item.id}`]);
    let parent = entry.parent;
    while (parent.kind === 'container') {
      const ancestor = folders.get(parent.id);
      if (ancestor === undefined || seen.has(`container:${parent.id}`)) break;
      seen.add(`container:${parent.id}`); parts.unshift(ancestor.basename); parent = ancestor.parent;
    }
    result.set(`${entry.item.kind}:${entry.item.id}`, parts.join('/'));
  }
  return result;
}

export function TreeActionDialog({ command, rows, directory, onClose, onDone }: {
  command: TreeAction; rows: readonly DocumentTreeListed[]; directory?: DocumentDirectoryContext | undefined; onClose: () => void; onDone?: () => void;
}) {
  const { t } = useTranslation();
  const key = useActKey();
  const [previewId] = useState(newActKey);
  const locale = useAppearance((state) => state.locale), timeZone = useAppearance((state) => state.timeZone);
  const first = rows[0];
  const renameFolder = command === 'rename' && first?.entry.item.kind === 'container';
  const [name, setName] = useState(command === 'create_folder' ? '' : command === 'rename' ? first?.entry.title ?? '' : first?.entry.basename ?? '');
  const [target, setTarget] = useState<Target | null>(directory === undefined ? null : { at: directory.at, label: directory.folder?.entry.title ?? t('documents.list.title') });
  const [standing, setStanding] = useState<DocumentDirectoryContext | null>(directory ?? null);
  const takesTarget = command === 'copy' || command === 'move';
  const takesName = command === 'create_folder' || command === 'rename' || rows.length === 1 && takesTarget;
  const base: Omit<DocumentTreeOperationBody, 'client_id' | 'deletion_plan'> = {
    command: renameFolder ? 'move' : command, items: rows.map((row) => row.entry.item),
    to: command === 'create_folder' ? directory?.at ?? null : renameFolder ? first.entry.parent : takesTarget ? target?.at ?? null : null,
    name: takesName && (!takesTarget || name.trim() !== first?.entry.basename) ? name.trim() || null : null, version: null,
  };
  const preview = useQuery({ queryKey: ['document-delete-preview', previewId, rows.map((row) => row.entry.item)],
    queryFn: () => previewDocumentDeletion({ ...base, client_id: key(base), deletion_plan: null }),
    enabled: command === 'delete', retry: false, staleTime: Infinity });
  const write = useMutation({
    mutationFn: () => organizeDocuments({ ...base, client_id: key(base), deletion_plan: command === 'delete' ? preview.data?.stamp ?? null : null }),
    onSuccess: (receipt) => {
      toast({ tone: 'success', title: t('documentFolders.done'),
        ...(receipt.gaining > 0 || receipt.losing > 0 ? { description: t('documentFolders.audience', { gaining: receipt.gaining, losing: receipt.losing }) } : {}) });
      onDone?.();
      onClose();
    },
  });
  const sameRoot = command !== 'move' || first !== undefined && standing !== null
    && first.entry.root === (standing.folder?.entry.root ?? standing.path[0]?.id);
  const targetAllowed = !takesTarget || target !== null && standing !== null
    && standing.at.kind === target.at.kind && standing.at.id === target.at.id && standing.can_create && sameRoot;
  const destination = base.to;
  const parentChanges = destination !== null && rows.some((row) => row.entry.parent.kind !== destination.kind || row.entry.parent.id !== destination.id);
  const placementChanges = command !== 'move' && !renameFolder || parentChanges
    || first !== undefined && base.name !== null && base.name !== first.entry.basename;
  const titleChanges = command !== 'rename' || renameFolder || first?.entry.title !== base.name;
  const allowed = placementChanges && titleChanges && (command !== 'create_folder' || directory?.can_create === true) && targetAllowed
    && (!(command === 'create_folder' || command === 'rename') || name.trim().length > 0)
    && (command !== 'delete' || preview.isSuccess);
  const plan = preview.data;
  const paths = plan === undefined ? new Map<string, string>() : deletionPaths(plan.stamp.entries);
  return <Dialog open onOpenChange={(open) => { if (!open && !write.isPending) onClose(); }} dismissible={!write.isPending}
    title={t(`documentFolders.actions.${command}`)} size="md" footer={<>
      <Button disabled={write.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant={command === 'delete' ? 'danger' : 'primary'} disabled={!allowed} pending={write.isPending} onClick={() => write.mutate()}>{command === 'copy' ? t('documentFolders.copy') : command === 'move' ? t('documentFolders.move') : t(`documentFolders.actions.${command}`)}</Button>
    </>}>
    <div className="flex flex-col gap-4">
      {command !== 'create_folder' && <p className="text-sm text-text-muted">{rows.map((row) => row.entry.basename).join(', ')}</p>}
      {takesTarget && <>
        <NamePicker label={t('people.destination')} orgId={command === 'move' ? first?.entry.root : undefined} chosen={target?.label ?? null} kinds={['org', 'project', 'topic', 'thread', 'document_container']}
          placeholder={t('documentFolders.findDestination')} nothing={t('documents.move.none')}
          onChoose={(hit) => { setStanding(null); setTarget(hit === null ? null : { label: hit.name, at: { kind: hit.kind === 'document_container' ? 'container' : 'node_docs', id: hit.id } }); }} />
        {command === 'move' && <p className="text-sm text-text-muted">{t('documentFolders.sameOrg')}</p>}
        {target !== null && <DestinationStanding target={target} onRead={setStanding} />}
        {standing !== null && !standing.can_create && <p className="text-sm text-text-muted">{t('people.destinationNeedsCreate')}</p>}
      </>}
      {takesName && <Input label={t('documentFolders.name')} value={name} onChange={(event) => setName(event.target.value)}
        hint={takesTarget ? t('documentFolders.nameHint') : undefined} />}
      {command === 'delete' && <>
        {preview.isPending ? <SkeletonRows rows={3} /> : preview.isError ? <ErrorAlert error={preview.error}
          action={<Button onClick={() => void preview.refetch()}>{t('common.tryAgain')}</Button>} /> : plan !== undefined && <>
          <p className="text-sm">{t('documentFolders.deleteCounts', {
            folders: t('documentFolders.folderCount', { count: plan.stamp.entries.filter((entry) => entry.item.kind === 'container').length }),
            documents: t('documentFolders.documentCount', { count: plan.stamp.entries.filter((entry) => entry.item.kind === 'document').length }),
            files: t('documentFolders.fileCount', { count: plan.stamp.entries.reduce((total, entry) => total + entry.files, 0) }),
            versions: t('documentFolders.versionCount', { count: plan.stamp.entries.reduce((total, entry) => total + entry.versions, 0) }),
          })}</p>
          <ul className="divide-y divide-border rounded-lg border border-border">{plan.stamp.entries.map((entry) => <li key={`${entry.item.kind}:${entry.item.id}`} className="px-3 py-2 text-sm">
            <p className="break-all">{paths.get(`${entry.item.kind}:${entry.item.id}`)}{entry.item.kind === 'container' ? '/' : ''}</p>
            {entry.item.kind === 'document' && <p className="text-xs text-text-muted">{t('documentFolders.retainedDetails', { files: t('documentFolders.fileCount', { count: entry.files }), versions: t('documentFolders.versionCount', { count: entry.versions }) })}</p>}
          </li>)}</ul>
          <p className="text-sm">{t('documentFolders.restoreUntil', { at: formatExact(plan.restore_until, { locale, timeZone }) })}</p>
        </>}
      </>}
      {write.isError && <ErrorAlert error={write.error} action={command === 'delete' ? <Button onClick={() => { write.reset(); void preview.refetch(); }}>{t('documentFolders.newPreview')}</Button> : undefined} />}
    </div>
  </Dialog>;
}
