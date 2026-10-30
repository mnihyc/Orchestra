/** Where a document lies and who may read it: moving it, deleting it, and making it private and open again.
 *
 * "Move to…" picks another node where the person may make documents and shows who gains and who loses
 * the document before it moves. Delete asks once. "Private: only you, until you share it" first shows who
 * will lose the document; "Open again" makes it follow its node once more, after showing who gains it.
 * Adding people to a document is Share, the permission page every other thing has.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { deleteDocument, moveDocument, previewMove, previewPrivate, setPrivate, type Document } from '@/api/documents';
import { holds } from '@/lib/permissions';
import { useOrgTree } from '@/realtime/stores/tree';
import { Button } from '@/ui/Button';
import { Choice } from '@/ui/Choice';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { cx } from '@/ui/cx';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';
import type { PagePreview } from '@/generated/api-types';

interface DialogProps {
  doc: Document;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** Who gains and who loses a document: the people the server names, a few of them, and how many more. */
function Preview({ preview }: { preview: PagePreview }) {
  const { t } = useTranslation();
  const names = (people: PagePreview['gaining']) => people.map((one) => one.user.display_name).join(', ');
  if (preview.gaining.length === 0 && preview.losing.length === 0) return <EmptyState compact title={t('documents.placement.nobody')} />;
  return (
    <div className="flex flex-col gap-1 text-sm">
      {preview.gaining.length > 0 && <p>{t('documents.placement.gain', { names: names(preview.gaining), more: preview.more })}</p>}
      {preview.losing.length > 0 && <p>{t('documents.placement.lose', { names: names(preview.losing), more: preview.more })}</p>}
    </div>
  );
}

// ---------------------------------------------------------------- private and open again

export function PrivateDialog({ doc, open, onOpenChange }: DialogProps) {
  const { t } = useTranslation();
  const whole = doc.page === 'restricted' ? 'reset' : 'private';
  const preview = useQuery({ queryKey: ['document-preview', doc.id, whole], queryFn: () => previewPrivate(doc.id, whole), enabled: open, staleTime: 0 });
  const change = useMutation({
    mutationFn: () => setPrivate(doc.id, whole),
    onSuccess: () => {
      onOpenChange(false);
    },
  });
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t(whole === 'private' ? 'documents.private.title' : 'documents.open.title')}
      confirmLabel={t(whole === 'private' ? 'documents.private.confirm' : 'documents.open.confirm')}
      pending={change.isPending}
      error={change.error}
      onConfirm={() => change.mutate()}
    >
      <div className="flex flex-col gap-2">
        <p>{t(whole === 'private' ? 'documents.private.about' : 'documents.open.about', { place: doc.node.name ?? '' })}</p>
        {preview.isPending ? <SkeletonRows rows={1} /> : preview.isError ? <ErrorAlert error={preview.error} action={<Button size="sm" onClick={() => void preview.refetch()}>{t('common.tryAgain')}</Button>} /> : <Preview preview={preview.data} />}
      </div>
    </ConfirmDialog>
  );
}

// ---------------------------------------------------------------- Delete

export function DeleteDialog({ doc, open, onOpenChange }: DialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const remove = useMutation({
    mutationFn: () => deleteDocument(doc.id),
    onSuccess: async () => {
      onOpenChange(false);
      toast({ tone: 'success', title: t('documents.delete.done', { title: doc.title }) });
      await navigate({ to: pathOf(doc.node.kind), params: { id: doc.node.id } as never } as never);
    },
  });
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={onOpenChange}
      title={t('documents.delete.title', { title: doc.title })}
      confirmLabel={t('documents.delete.confirm', { title: doc.title })}
      danger
      pending={remove.isPending}
      error={remove.error}
      onConfirm={() => remove.mutate()}
    >
      {t('documents.delete.about')}
    </ConfirmDialog>
  );
}

/** The screen of a node, by its kind. */
function pathOf(kind: string): string {
  return kind === 'project' ? '/project/$id' : kind === 'topic' ? '/topic/$id' : kind === 'thread' ? '/thread/$id' : '/org/$id';
}

// ---------------------------------------------------------------- Move

export function MoveDialog({ doc, open, onOpenChange }: DialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const root = doc.path[0]?.id ?? null;
  const tree = useOrgTree(root);
  const [query, setQuery] = useState('');
  const [target, setTarget] = useState<string | null>(null);
  const places = useMemo(
    () =>
      [...tree.nodes.values()]
        .filter((node) => !node.deleted && !node.archived && node.id !== doc.node.id)
        .filter((node) => node.name.toLowerCase().includes(query.trim().toLowerCase()))
        .slice(0, 30),
    [tree.nodes, query, doc.node.id],
  );
  const preview = useQuery({ queryKey: ['document-move', doc.id, target], queryFn: () => previewMove(doc.id, target ?? ''), enabled: open && target !== null, staleTime: 0 });
  const move = useMutation({
    mutationFn: () => moveDocument(doc.id, target ?? ''),
    onSuccess: (moved) => {
      toast({ tone: 'success', title: t('documents.move.done', { place: moved.node.name ?? '' }) });
      onOpenChange(false);
      void navigate({ to: '/doc/$documentId', params: { documentId: doc.id } });
    },
  });
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={t('documents.move.title', { title: doc.title })}
      description={t('documents.move.about')}
      size="md"
      footer={
        <>
          <Button variant="secondary" onClick={() => onOpenChange(false)}>
            {t('common.cancel')}
          </Button>
          <Button variant="primary" disabled={target === null || !holds(doc.access, 'doc.manage')} pending={move.isPending} onClick={() => move.mutate()}>
            {t('documents.move.confirm')}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <Input label={t('documents.move.search')} value={query} onChange={(event) => setQuery(event.target.value)} />
        <div className="max-h-(--doc-menu-height) overflow-y-auto rounded-lg border border-border">
          {places.length === 0 && <EmptyState compact className="px-3" title={t('documents.move.none')} />}
          <div role="listbox" aria-label={t('documents.move.places')} className="flex flex-col">
            {places.map((node) => (
              <Choice presentation="picker" key={node.id} name={node.name} description={t(`documents.move.kind_${node.kind}` as 'documents.move.kind_project')} compact>{(content, control) => <button
                {...control}
                type="button"
                role="option"
                aria-selected={target === node.id}
                className={cx('flex w-full items-center justify-between gap-2 px-3 py-1 text-left text-sm hover:bg-bg-muted', target === node.id && 'bg-accent-subtle')}
                onClick={() => setTarget(node.id)}
              >
                {content}
              </button>}</Choice>
            ))}
          </div>
        </div>
        {target !== null && (preview.isPending ? <SkeletonRows rows={1} /> : preview.isError ? <ErrorAlert error={preview.error} action={<Button size="sm" onClick={() => void preview.refetch()}>{t('common.tryAgain')}</Button>} /> : <Preview preview={preview.data} />)}
        {move.isError && <ErrorAlert error={move.error} />}
      </div>
    </Dialog>
  );
}
