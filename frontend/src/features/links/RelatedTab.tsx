/** Related: the documents, chats and threads related by meaning, place and references, a page at a time. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { relatedPagesQuery } from '@/api/links';
import { RelatedRow } from '@/features/inspector/Related';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';

const LOADING_ROWS = 3;

export function RelatedTab({ documentId, seq, enabled }: { documentId: string; seq: number; enabled: boolean }) {
  const { t } = useTranslation();
  const query = useInfiniteQuery({ ...relatedPagesQuery(documentId, seq), enabled });
  const items = query.data?.pages.flatMap((page) => page.items) ?? [];
  const meaning = query.data?.pages[0]?.meaning ?? 'ready';
  const labels = usePlaceLabels(items.map((item) => ({ id: item.id, name: item.name || t(`related.kinds.${item.kind}`), above: item.context })));
  if (!enabled) return null;
  if (query.isPending) return <SkeletonRows rows={LOADING_ROWS} />;
  if (query.isError && items.length === 0) return <ErrorAlert error={query.error} action={<Button size="sm" onClick={() => void query.refetch()}>{t('common.tryAgain')}</Button>} />;
  return <div className="flex min-w-0 flex-col" data-links-list="related">
    {meaning !== 'ready' && <p className="px-2 pb-1 text-sm text-text-muted" role="status">{t(`related.meaning.${meaning}`)}</p>}
    {items.length > 0 && <ul className="flex min-w-0 flex-col gap-1" aria-label={t('related.title')}>
      {items.map((item) => <RelatedRow key={`${item.kind}:${item.id}`} item={item} label={labels.get(item.id)} />)}
    </ul>}
    <Pagination autoLoad hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : undefined} onMore={() => void query.fetchNextPage({ cancelRefetch: false })} />
  </div>;
}
