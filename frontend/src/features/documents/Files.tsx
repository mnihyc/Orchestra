/** The files a document holds, against its limits, each opening in the code view.
 *
 * Everything the document holds beside its text: images, films, data sets, scripts, of any kind, with
 * their sizes against what the place allows.
 */
import { useMutation } from '@tanstack/react-query';
import { Download, FileText, Plus, Trash2 } from 'lucide-react';
import { useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { api, done } from '@/api/client';
import type { Document } from '@/api/documents';
import { UploadChips } from '@/features/composer/Attachments';
import { attachDocumentUpload } from '@/features/documents/Media';
import type { DocumentFile } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { useUploads } from '@/lib/useUploads';
import { documentStateNow, documentWriter } from '@/realtime/stores/document';
import { Button } from '@/ui/Button';
import { ByteSize } from '@/ui/ByteSize';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert } from '@/ui/ErrorState';

import { FileAttachment } from '@/features/files/FileChip';
import { FileOverlay } from '@/features/files/FileOverlay';
import { documentSource, sourceDownload } from '@/features/files/source';
import { useChipPresentation } from '@/features/people/chipSizes';

export function FilesDialog({ doc, open, fileId, line, onOpenChange }: { doc: Document; open: boolean; fileId?: string | undefined; line?: number | undefined; onOpenChange: (open: boolean) => void }) {
  const { t } = useTranslation();
  const picker = useRef<HTMLInputElement>(null);
  const shown = doc.files.find((file) => file.id === fileId) ?? null;
  const inspection = doc.inspection !== undefined;
  const { size } = useChipPresentation({ place: 'compact' });
  const mayEdit = holds(doc.access, 'doc.edit') && !doc.archived && !inspection;
  const uploads = useUploads(doc.id, (blob, signal) => attachDocumentUpload(doc.id, blob, signal), () => undefined);
  const remove = useMutation({
    mutationFn: (file: DocumentFile) => done(api.DELETE('/documents/{id}/files/{file_id}', { params: { path: { id: doc.id, file_id: file.id } } })),
    onSuccess: (_, file) => {
      const state = documentStateNow(doc.id);
      if (state.document !== null) {
        const files = state.document.files.filter((held) => held.id !== file.id);
        documentWriter.apply(doc.id, { document: { ...state.document, files, files_bytes: files.reduce((sum, held) => sum + held.size, 0) } }, state.lastSeq);
      }
    },
  });
  const download = useMutation({ mutationFn: (file: DocumentFile) => sourceDownload(documentSource(doc.id, file, inspection)) });
  return (
    <>
    <Dialog open={open} onOpenChange={onOpenChange} title={t('documents.files.title')} size="md" footer={<Button onClick={() => onOpenChange(false)}>{t('common.close')}</Button>}>
      <div className="flex flex-col gap-3">
        {mayEdit && <>
          <input ref={picker} type="file" multiple hidden aria-label={t('uploads.choose')} onChange={(event) => { if (event.target.files !== null) uploads.add(event.target.files); event.target.value = ''; }} />
          <Button size="sm" icon={<Plus aria-hidden />} onClick={() => picker.current?.click()}>{t('uploads.add')}</Button>
          <UploadChips queue={uploads} />
        </>}
        {remove.isError && <ErrorAlert error={remove.error} />}
        {download.isError && <ErrorAlert error={download.error} />}
        <div
          role="progressbar"
          aria-label={t('documents.files.used')}
          aria-valuemin={0}
          aria-valuemax={doc.limits.files_bytes}
          aria-valuenow={doc.files_bytes}
          className="h-2 overflow-hidden rounded-full bg-bg-muted"
        >
          <div className="h-full rounded-full bg-accent" style={{ width: `${Math.min(100, (doc.files_bytes / Math.max(1, doc.limits.files_bytes)) * 100)}%` }} />
        </div>
        <p className="text-sm text-text-muted">
          <ByteSize bytes={doc.files_bytes} /> / <ByteSize bytes={doc.limits.files_bytes} /> · {t('documents.files.most')} <ByteSize bytes={doc.limits.file_bytes} />
        </p>
        {doc.files.length === 0 ? (
          <EmptyState compact icon={<FileText aria-hidden />} title={t('documents.files.none')} />
        ) : (
          <ul className="flex flex-col divide-y divide-border rounded-lg border border-border">
            {doc.files.map((file) => (
              <li key={file.id} className="flex flex-wrap items-center gap-2 px-3 py-2 text-sm">
                <div className="min-w-0 flex-1"><FileAttachment source={documentSource(doc.id, file, inspection)} presentation={{ place: 'compact' }} /></div>
                {size === 'small' && <ByteSize bytes={file.size} className="text-text-muted" />}
                <Button size="sm" icon={<Download aria-hidden />} pending={download.isPending && download.variables.id === file.id} onClick={() => download.mutate(file)}>
                  {t('documents.media.download')}
                </Button>
                {mayEdit && <Button size="sm" variant="ghost" aria-label={t('uploads.delete', { name: file.name })} icon={<Trash2 aria-hidden />} disabled={remove.isPending} onClick={() => remove.mutate(file)} />}
              </li>
            ))}
          </ul>
        )}
      </div>
    </Dialog>
    {open && shown !== null && <FileOverlay source={{ ...documentSource(doc.id, shown, inspection), line }} onClose={() => onOpenChange(false)} />}
    </>
  );
}
