/** The dialogs of a document that stands in the tree: a new title, another place, and deleting it with the way back.
 *
 * They are opened from the document's menu in the left bar and drawn by the one host of the frame.
 * Moving is the documents' own dialog, which shows who gains and who loses the document before it
 * moves. Renaming and deleting leave the reader where they are: only somebody who was looking at the
 * document that was deleted is taken to the place it stood beside.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useNavigate, useRouterState } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';

import { deleteDocument, documentQuery, renameDocument, restoreDocument } from '@/api/documents';
import { MoveDialog } from '@/features/documents/Placement';
import { type DocPlace, documentPath, placePath } from '@/features/navigation/places';
import { useForm } from '@/lib/forms';
import { Button } from '@/ui/Button';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { adminSpec } from '@/ui/AdminAct';
import { Dialog } from '@/ui/Dialog';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { SkeletonRows } from '@/ui/Skeleton';
import { toast } from '@/ui/Toast';

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
      {whole.isError ? <ErrorState compact error={whole.error} retrying={whole.isRefetching} onRetry={() => void whole.refetch()} /> : <SkeletonRows rows={3} className="py-2" />}
    </Dialog>
  );
}

export function DeleteDocumentDialog({ doc, open, onClose }: DialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const viewing = useRouterState({ select: (state) => state.location.pathname === documentPath(doc.id) });
  const remove = useMutation({
    mutationFn: () => deleteDocument(doc.id),
    onSuccess: () => {
      onClose();
      // Whoever was looking at it has nothing left to look at: they go to the place it stood beside.
      if (viewing) void navigate({ to: placePath(doc.node) });
      toast({
        tone: 'success',
        title: t('documents.delete.done', { title: doc.title }),
        action: { label: t('places.toast.undo'), run: () => void restoreDocument(doc.id) },
      });
    },
  });
  return (
    <ConfirmDialog
      open={open}
      onOpenChange={(next) => !next && onClose()}
      title={t('documents.delete.title', { title: doc.title })}
      admin={adminSpec(doc, 'doc.manage')}
      confirmLabel={t('documents.delete.confirm', { title: doc.title })}
      danger
      pending={remove.isPending}
      error={remove.error ?? undefined}
      onConfirm={() => remove.mutate()}
    >
      {t('documents.delete.about')}
    </ConfirmDialog>
  );
}
