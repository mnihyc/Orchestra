/** An attachment locator opens the existing viewer after checking its reference and owning chat. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { referencePreviewQuery } from '@/api/references';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { SkeletonRows } from '@/ui/Skeleton';
import { FileOverlay } from './FileOverlay';
import { FileState, fileFailure } from './FilePreview';
import { referenceSource } from './source';

export function ReferenceFile({ id, chatId, line, endLine, onClose }: {
  id: string; chatId: string; line?: number | undefined; endLine?: number | undefined; onClose: () => void;
}) {
  const { t } = useTranslation();
  const preview = useQuery(referencePreviewQuery({ kind: 'file', id, ...(line === undefined ? {} : { line }) }));
  const data = preview.data;
  const permitted = data !== undefined && (data.unavailable === null || data.unavailable === undefined) && data.file?.owner_kind === 'chat' && data.file.owner_id === chatId;
  const source = permitted ? referenceSource(data) : null;
  if (source !== null) return <FileOverlay source={{ ...source, line, endLine }} onClose={onClose} />;
  return <Dialog open title={t('fileMedia.open')} onOpenChange={(open) => { if (!open) onClose(); }}
    footer={<Button onClick={onClose}>{t('common.close')}</Button>}>
    {preview.isPending ? <SkeletonRows rows={3} /> : <FileState state={preview.isError ? fileFailure(preview.error) : (data?.unavailable !== null && data?.unavailable !== undefined) ? 'private' : 'gone'} />}
  </Dialog>;
}
