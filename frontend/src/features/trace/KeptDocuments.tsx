/** The same guarded retained document rows in Recently deleted and a document list. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { FileText } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { keptDocumentsQuery } from '@/api/trace';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { ObjectActs } from './ObjectActs';
import type { KeptDocumentRef } from './pending';

export function KeptDocumentRow({ doc }: { doc: KeptDocumentRef }) {
  return <div className="flex min-w-0 flex-col gap-2" data-kept-document={doc.id}><p className="font-medium wrap-anywhere">{doc.title}</p><ObjectActs object={{ id: doc.id, kind: 'document', name: doc.title, state: 'deleted', deletion_id: doc.deletion_id ?? null, deleted_at: doc.deleted_at ?? null, deleted_by: doc.deleted_by ?? null, purge_at: doc.purge_at ?? null, purged_at: null, can_open: false, can_inspect: doc.can_inspect === true, can_restore: doc.can_restore === true }} /></div>;
}
export function KeptDocuments({ node, open, hideEmpty = false }: { node: string; open: boolean; hideEmpty?: boolean }) {
  const { t } = useTranslation(); const list = useInfiniteQuery({ ...keptDocumentsQuery(node), enabled: open }); const rows = list.isError ? [] : list.data?.pages.flatMap((page) => page.items) ?? [];
  if (hideEmpty && list.isSuccess && rows.length === 0) return null;
  return <section aria-label={t('trace.document')} className="mt-6 flex flex-col gap-3"><h3 className="font-medium">{t('trace.document')}</h3>{list.isPending ? <SkeletonRows rows={3} /> : list.isError ? <ErrorState compact title={t('trace.documentReadFailure')} error={list.error} onRetry={() => void list.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('documents.list.noneDeleted')} icon={<FileText aria-hidden />} /> : <ul className="divide-y divide-border">{rows.map((doc) => <li key={doc.id} className="py-4"><KeptDocumentRow doc={doc} /></li>)}</ul>}<Pagination hasMore={list.hasNextPage} loading={list.isFetchingNextPage} error={list.isFetchNextPageError ? list.error : undefined} onMore={() => void list.fetchNextPage()} /></section>;
}
