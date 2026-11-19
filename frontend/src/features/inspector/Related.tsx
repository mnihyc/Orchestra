/** A document inspector's bounded related list, read once per revision; reference cards load only when opened. */
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';

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
  const options = relatedQuery(documentId, seq);
  const read = useInfiniteQuery({ ...options, enabled });
  if (!enabled) return null;
  if (read.isPending) return <SkeletonRows rows={LOADING_ROWS} />;
  if (read.isError) return <ErrorAlert error={read.error} action={<Button size="sm" onClick={() => void cache.resetQueries({ queryKey: options.queryKey, exact: true })}>{t('common.tryAgain')}</Button>} />;
  const first = read.data.pages[0];
  if (first === undefined) return null;
  const items = read.data.pages.flatMap((page) => page.items);
  if (items.length === 0) return null;
  return <RelatedList answer={{ ...first, items }} more={read.hasNextPage} pending={read.isFetchingNextPage}
    next={() => void read.fetchNextPage()} />;
}

function RelatedList({ answer, more, pending, next }: { answer: RelatedItems; more: boolean; pending: boolean; next: () => void }) {
  const { t } = useTranslation();
  const labels = usePlaceLabels(answer.items.map((item) => ({ id: item.id, name: item.name || t(`related.kinds.${item.kind}`), above: item.context })));
  return <div className="h-full min-w-0 overflow-y-auto p-2" data-related-items="">
    {answer.meaning !== 'ready' && <p className="px-2 pb-1 text-sm text-text-muted" role="status">{t(`related.meaning.${answer.meaning}`)}</p>}
    <ul className="flex min-w-0 flex-col gap-1" aria-label={t('related.title')}>{answer.items.map((item) => <RelatedRow key={`${item.kind}:${item.id}`} item={item} label={labels.get(item.id)} />)}</ul>
    {more && <Button size="sm" disabled={pending} onClick={next}>{pending ? t('common.loading') : t('related.more')}</Button>}
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
