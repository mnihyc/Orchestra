/** The dialogs of a document that stands in the tree: a new title and another place; Delete moves it to the trash at once.
 *
 * They are opened from the document's menu in the left bar and drawn by the one host of the frame.
 * Moving is the documents' own dialog, which shows who gains and who loses the document before it
 * moves. Renaming and deleting leave the reader where they are: only somebody who was looking at the
 * document that was deleted is taken to the place it stood beside.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useEffect, useRef } from 'react';
import { useNavigate, useRouterState } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { documentQuery, renameDocument } from '@/api/documents';
import { MoveDialog } from '@/features/documents/Placement';
import { type DocPlace, documentPath, placePath } from '@/features/navigation/places';
import { useDocumentTrash } from '@/features/trash/useDocumentTrash';
import { placeById } from '@/features/trash/where';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';

interface DialogProps {
  doc: DocPlace;
  open: boolean;
  onClose: () => void;
}

/** The most characters a title holds, as the page of a document allows them. */
const TITLE_MAX = 200;

export function RenameDocumentDialog({ doc, open, onClose }: DialogProps) {
  const { t } = useTranslation();
  const form = useForm({ title: doc.title }, { title: (value) => (value.trim() === '' ? t('navigation.doc.titleMissing') : null) });
  const save = useMutation({
    mutationFn: (title: string) => renameDocument(doc.id, title),
    onSuccess: onClose,
    onError: (error) => void form.refuse(error),
  });
  return (
    <Dialog
      open={open}
      onOpenChange={(next) => !next && !save.isPending && onClose()}
      size="sm"
      title={t('places.rename.title', { name: t('places.named.root', { name: doc.title }) })}
      dismissible={!save.isPending}
      footer={
        <>
          <Button disabled={save.isPending} onClick={onClose}>
            {t('common.cancel')}
          </Button>
          <Button type="submit" form="rename-document" variant="primary" pending={save.isPending}>
            {t('places.rename.submit')}
          </Button>
        </>
      }
    >
      <form
        id="rename-document"
        noValidate
        className="flex flex-col gap-4 py-2"
        onSubmit={form.submit((values) => {
          if (save.isPending) return;
          const title = values.title.trim();
          if (title === doc.title) onClose();
          else save.mutate(title);
        })}
      >
        <Input label={t('navigation.doc.title')} autoComplete="off" maxLength={TITLE_MAX} {...form.field('title')} />
        {save.error !== null && <ErrorAlert error={save.error} />}
      </form>
    </Dialog>
  );
}

/** Moving needs the document whole (what the caller holds on it, the org it lies in): it is read first, and is at hand already where its menu was just open. */
export function MoveDocumentDialog({ doc, open, onClose }: DialogProps) {
  const { t } = useTranslation();
  const whole = useQuery(documentQuery(doc.id));
  if (whole.data !== undefined) return <MoveDialog doc={whole.data} open={open} onOpenChange={(next) => !next && onClose()} />;
  return (
    <Dialog open={open} onOpenChange={(next) => !next && onClose()} size="md" title={t('documents.move.title', { title: doc.title })}>
      {whole.isError ? <ErrorState compact error={whole.error} retrying={whole.isRefetching} onRetry={() => void whole.refetch()} /> : <SkeletonRows read={whole} rows={3} className="py-2" />}
    </Dialog>
  );
}

/**
 * Deleting asks nothing: the document goes to the trash of its place, and the line after it has Show and Undo.
 * Whoever was looking at it has nothing left to look at and goes to the place it stood beside.
 */
export function TrashDocument({ request, doc, onClose }: { request: object; doc: DocPlace; onClose: () => void }) {
  const navigate = useNavigate();
  const viewing = useRouterState({ select: (state) => state.location.pathname === documentPath(doc.id) });
  const trash = useDocumentTrash();
  // Each opening is one act, however often the host draws it.
  const asked = useRef<object | null>(null);
  useEffect(() => {
    if (asked.current === request) return;
    asked.current = request;
    onClose();
    void trash([{ item: { kind: 'document', id: doc.id }, name: doc.title }], placeById(doc.node.id, doc.node.kind)).then((result) => {
      if (viewing && result.groups.length > 0) void navigate({ to: placePath(doc.node) });
    });
  }, [request, doc, trash, onClose, navigate, viewing]);
  return null;
}
