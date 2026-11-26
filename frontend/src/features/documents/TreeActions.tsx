/** One document operation: folder creation, copy, move and rename in their dialog; Delete moves the rows to the trash at once. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { documentDirectoryQuery, documentTreeEntryQuery, organizeDocuments } from '@/api/documentTree';
import { documentAncestors } from '@/features/documents/treeLocation';
import { useDocumentTrash } from '@/features/trash/useDocumentTrash';
import { placeById } from '@/features/trash/where';
import { DocumentMoveDialog } from './DocumentMoveDialog';
import { WAY_SEPARATOR } from '@/features/finder/destinations';
import { NamePicker } from '@/features/search/NamePicker';
import type { DocumentDirectory, DocumentDirectoryContext, DocumentTreeListed, DocumentTreeOperationBody } from '@/generated/api-types';
import { useActKey } from '@/lib/forms';
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
  if (query.isPending) return <SkeletonRows read={query} rows={1} />;
  if (value === undefined || value === null) return null;
  const names = [...value.path.map((step) => step.redacted ? t('ui.nameUnknown') : step.name ?? t('ui.nameUnknown')), t('documents.list.title'),
    ...documentAncestors(value.folder?.entry.parent ?? value.at, value.ancestors).map((step) => step.title ?? t('ui.nameUnknown')),
    ...(value.folder === null ? [] : [value.folder.entry.title])];
  return <p className="break-words text-sm text-text-muted">{names.join(WAY_SEPARATOR)}</p>;
}

interface TreeActionDialogProps {
  command: TreeAction; rows: readonly DocumentTreeListed[]; directory?: DocumentDirectoryContext | undefined; onClose: () => void; onDone?: () => void;
}
export function TreeActionDialog(props: TreeActionDialogProps) {
  const first = props.rows[0];
  if (props.command === 'delete') return <TrashRows {...props} />;
  if (props.command === 'move' && props.rows.length === 1 && first !== undefined) return <DocumentMoveDialog row={first} directory={props.directory} onClose={props.onClose} onDone={props.onDone} />;
  return <OrdinaryTreeActionDialog {...props} />;
}

/** Delete asks nothing: the rows go to the trash of their place in one act, and the line after it has Show and Undo. */
function TrashRows({ rows, onClose, onDone }: TreeActionDialogProps) {
  const trash = useDocumentTrash();
  const asked = useRef(false);
  useEffect(() => {
    if (asked.current) return;
    asked.current = true;
    onClose();
    const first = rows[0];
    void trash(rows.map((row) => ({ item: row.entry.item, name: row.entry.title })), first === undefined ? null : placeById(first.entry.node)).then((result) => {
      if (result.groups.length > 0) onDone?.();
    });
  }, [rows, trash, onClose, onDone]);
  return null;
}
function OrdinaryTreeActionDialog({ command, rows, directory, onClose, onDone }: TreeActionDialogProps) {
  const { t } = useTranslation();
  const key = useActKey();
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
  const write = useMutation({
    mutationFn: () => organizeDocuments({ ...base, client_id: key(base), deletion_plan: null }),
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
    && (!(command === 'create_folder' || command === 'rename') || name.trim().length > 0);
  return <Dialog open onOpenChange={(open) => { if (!open && !write.isPending) onClose(); }} dismissible={!write.isPending}
    title={t(`documentFolders.actions.${command}`)} size="md" footer={<>
      <Button disabled={write.isPending} onClick={onClose}>{t('common.cancel')}</Button>
      <Button variant="primary" disabled={!allowed} pending={write.isPending} onClick={() => write.mutate()}>{command === 'copy' ? t('documentFolders.copy') : command === 'move' ? t('documentFolders.move') : t(`documentFolders.actions.${command}`)}</Button>
    </>}>
    <div className="flex flex-col gap-4">
      {command !== 'create_folder' && <p className="text-sm text-text-muted">{rows.map((row) => row.entry.basename).join(', ')}</p>}
      {takesTarget && <>
        <NamePicker pickerKey="documents.TreeActions.people.destination" label={t('people.destination')} orgId={command === 'move' ? first?.entry.root : undefined} chosen={target?.label ?? null} kinds={['org', 'project', 'topic', 'thread', 'document_container']}
          placeholder={t('documentFolders.findDestination')} nothing={t('documents.move.none')}
          onChoose={(hit) => { setStanding(null); setTarget(hit === null ? null : { label: hit.name, at: { kind: hit.kind === 'document_container' ? 'container' : 'node_docs', id: hit.id } }); }} />
        {command === 'move' && <p className="text-sm text-text-muted">{t('documentFolders.sameOrg')}</p>}
        {target !== null && <DestinationStanding target={target} onRead={setStanding} />}
        {standing !== null && !standing.can_create && <p className="text-sm text-text-muted">{t('people.destinationNeedsCreate')}</p>}
      </>}
      {takesName && <Input label={t('documentFolders.name')} value={name} onChange={(event) => setName(event.target.value)}
        hint={takesTarget ? t('documentFolders.nameHint') : undefined} />}
      {write.isError && <ErrorAlert error={write.error} />}
    </div>
  </Dialog>;
}
