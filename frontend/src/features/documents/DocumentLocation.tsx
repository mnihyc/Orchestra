/** Folder placement has a metadata feed because renaming an ancestor does not edit document content. */
import { useTranslation } from 'react-i18next';
import { documentAncestors, documentsPath, folderPath } from '@/features/documents/treeLocation';
import type { DocumentTreeRead } from '@/generated/api-types';
import { Breadcrumbs } from '@/ui/Breadcrumbs';
import { ErrorState } from '@/ui/ErrorState';

export function DocumentLocation({ metadata, error, onRetry }: { metadata: DocumentTreeRead | undefined; error: unknown; onRetry: () => void }) {
  const { t } = useTranslation();
  if (error !== null && error !== undefined) return <ErrorState compact error={error} onRetry={onRetry} />;
  const row = metadata?.entries[0];
  if (row === undefined || row.entry.parent.kind === 'node_docs') return null;
  const place = metadata?.place?.node ?? row.document?.node;
  const crumbs = [
    { id: `docs:${row.entry.node}`, label: t('documents.list.title'), href: place?.redacted === false ? documentsPath(row.entry.node) : undefined },
    ...documentAncestors(row.entry.parent, metadata?.ancestors ?? []).map((ancestor) => ({
      id: ancestor.id, label: ancestor.title ?? t('ui.nameUnknown'), href: ancestor.standing.read ? folderPath(ancestor.id) : undefined,
    })),
  ];
  return <div className="mb-3"><Breadcrumbs crumbs={crumbs} /></div>;
}
