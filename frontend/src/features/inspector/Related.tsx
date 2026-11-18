/** A document inspector's bounded related list, read once per revision; reference cards load only when opened. */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

import { useCollectionSearch } from '@/lib/collectionSearch';
import { usePickerCursorRestart } from '@/lib/pickerPages';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { StateLine } from '@/ui/StateLine';
import { relatedQuery } from '@/api/search';
import { NamedChip } from '@/features/people/NamedChip';
import { usePlaceLabels } from '@/features/navigation/placeLabels';
import type { RelatedItem, RelatedItems } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

/** Three short placeholders show the list's shape without reserving a whole inspector for pending rows. */
const LOADING_ROWS = 3;

export function RelatedPanel({ documentId, seq, enabled = true }: { documentId: string; seq: number; enabled?: boolean }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const search = useCollectionSearch(`related:${documentId}`);
  const options = relatedQuery(documentId, seq, search.q);
  const read = useInfiniteQuery({ ...options, enabled });
  usePickerCursorRestart(read, options.queryKey);
  if (!enabled) return null;
  const first = read.data?.pages[0];
  const items = read.data?.pages.flatMap((page) => page.items) ?? [];
  return <div className="flex h-full min-w-0 flex-col gap-2 overflow-y-auto p-2" data-related-items="">
    <CollectionSearch name={t('related.title')} {...search} />
    {read.isPending ? <SkeletonRows read={read} rows={LOADING_ROWS} /> : read.isError && !read.isFetchNextPageError
      ? <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void cache.resetQueries({ queryKey: options.queryKey, exact: true })}>{t('common.tryAgain')}</Button>} />
      : first === undefined ? null : <>
        {items.length === 0 && search.q !== '' && <StateLine compact title={t('ui.combobox.empty')} />}
        <RelatedList answer={{ ...first, items }} more={read.hasNextPage} pending={read.isFetchingNextPage} error={read.isFetchNextPageError ? read.error : undefined}
          next={() => void read.fetchNextPage()} />
      </>}
  </div>;
}

function RelatedList({ answer, more, pending, next, error }: { answer: RelatedItems; more: boolean; pending: boolean; next: () => void; error?: unknown }) {
  const { t } = useTranslation();
  const labels = usePlaceLabels(answer.items.map((item) => ({ id: item.id, name: item.name || t(`related.kinds.${item.kind}`), above: item.context })));
  return <div className="flex min-w-0 flex-col">
    {answer.meaning !== 'ready' && <p className="px-2 pb-1 text-sm text-text-muted" role="status">{t(`related.meaning.${answer.meaning}`)}</p>}
    {answer.items.length > 0 && <ul className="flex min-w-0 flex-col gap-1" aria-label={t('related.title')}>{answer.items.map((item) => <RelatedRow key={`${item.kind}:${item.id}`} item={item} label={labels.get(item.id)} />)}</ul>}
    <Pagination autoLoad hasMore={more} loading={pending} error={error} onMore={next} />
  </div>;
}

export function RelatedRow({ item, label }: { item: RelatedItem; label?: string | undefined }) {
  const { t } = useTranslation();
  const kind = item.kind === 'document' ? 'doc' : item.kind;
  const name = label ?? (item.name || t(`related.kinds.${item.kind}`));
  const why = [
    item.place === null ? null : t(`related.place.${item.place}`),
    item.related_by.includes('meaning') ? t('related.similarMeaning') : null,
    item.references === null ? null : t(`related.references.${item.references}`),
  ].filter((part): part is string => part !== null).join(' · ');
  return <li className="flex min-h-(--row-height) min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-md px-2 py-1 hover:bg-bg-subtle">
    <NamedChip id={item.id} kind={kind} path={`/${kind}/${item.id}`} name={name} presentation={{ place: 'compact' }} />
    <span className="max-w-full text-sm text-text-muted wrap-anywhere">{why}</span>
  </li>;
}
