/** The document editor uses the same typed document-tree operations as the folder browser. */
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { useNavigate } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import { documentTreeEntryQuery } from '@/api/documentTree';
import { TreeActionDialog } from '@/features/documents/TreeActions';
import { Dialog } from '@/ui/Dialog';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function DocumentOrganize({ id, command, onClose }: { id: string; command: 'copy' | 'move' | 'delete'; onClose: () => void }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const navigate = useNavigate();
  const read = useQuery(documentTreeEntryQuery({ kind: 'document', id }, cache));
  const row = read.data?.entries.find((entry) => entry.entry.item.kind === 'document' && entry.entry.item.id === id);
  if (row === undefined || read.isError) return <Dialog open title={t(`documentFolders.actions.${command}`)} onOpenChange={(open) => { if (!open) onClose(); }}>
    {read.isPending ? <SkeletonRows rows={2} /> : <ErrorState compact error={read.error ?? new Error('The document metadata omitted its entry')}
      onRetry={() => void read.refetch()} />}
  </Dialog>;
  return <TreeActionDialog command={command} rows={[row]} onClose={onClose} onDone={() => {
    if (command === 'delete') void navigate({ to: '/inspect/$kind/$id', params: { kind: 'document', id } });
  }} />;
}
