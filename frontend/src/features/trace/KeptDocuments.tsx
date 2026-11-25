/** The same guarded retained document rows in Recently deleted and a document list. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { FileText } from 'lucide-react';
import { useCallback } from 'react';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { useRetainedRows } from '@/lib/retainedRows';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { useTranslation } from 'react-i18next';
import { keptDocumentsQuery } from '@/api/trace';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { ObjectActs } from './ObjectActs';
import type { KeptDocumentRef } from './pending';

export function KeptDocumentRow({ doc, keep }: { doc: KeptDocumentRef; keep?: (doc: KeptDocumentRef, active: boolean) => void }) {
  const onActive = useCallback((active: boolean) => keep?.(doc, active), [keep, doc]);
  return <div className="flex min-w-0 flex-col gap-2" data-kept-document={doc.id}><p className="font-medium wrap-anywhere">{doc.title}</p><ObjectActs onActive={onActive} object={{ id: doc.id, kind: 'document', name: doc.title, state: 'deleted', deletion_id: doc.deletion_id ?? null, deleted_at: doc.deleted_at ?? null, deleted_by: doc.deleted_by ?? null, purge_at: doc.purge_at ?? null, purged_at: null, can_open: false, can_inspect: doc.can_inspect === true, can_restore: doc.can_restore === true }} /></div>;
}
export function KeptDocuments({ node, open, hideEmpty = false }: { node: string; open: boolean; hideEmpty?: boolean }) {
  const search = useCollectionSearch('kept-documents:' + node);
  return <KeptDocumentList key={search.scope} node={node} open={open} hideEmpty={hideEmpty} search={search} />;
}
function KeptDocumentList({ node, open, hideEmpty, search }: { node: string; open: boolean; hideEmpty: boolean; search: ReturnType<typeof useCollectionSearch> }) {
  const { t } = useTranslation();
  const list = useInfiniteQuery({ ...keptDocumentsQuery(node, search.q), enabled: open });
  const rows = list.isError ? [] : list.data?.pages.flatMap((page) => page.items) ?? [];
  const kept = useRetainedRows(rows);
  if (hideEmpty && search.text === '' && list.isSuccess && rows.length === 0 && kept.rows.length === 0) return null;
  return <section aria-label={t('trace.document')} className="mt-6 flex flex-col gap-3">
    <h3 className="font-medium">{t('trace.document')}</h3>
    <CollectionSearch name={t('trace.document')} {...search} />
    {list.isPending ? <SkeletonRows read={list} rows={3} /> : list.isError ? <ErrorState compact title={t('trace.documentReadFailure')} error={list.error} onRetry={() => void list.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('documents.list.noneDeleted')} icon={<FileText aria-hidden />} /> : null}
    <ScrollRegion kind="table" label={t('trace.document')} className="max-h-96 overflow-y-auto"><ul className="divide-y divide-border">
      {kept.rows.map((doc) => <li key={doc.id} hidden={!kept.visible.has(doc.id)} className="py-4"><KeptDocumentRow doc={doc} keep={kept.keep} /></li>)}
    </ul></ScrollRegion>
    <Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} />
  </section>;
}
