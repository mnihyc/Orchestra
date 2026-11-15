/** Complete native declarations and parent-owned plan operands stay whole while their rows are found and paged. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { EmptyState } from '@/ui/EmptyState';
import { Pagination } from '@/ui/Pagination';
import { ScrollRegion } from '@/ui/ScrollRegion';

/** Match an ordinary server batch without turning that batch into a total limit. */
const PAGE_ROWS = 50;

export function CompleteCollection<Item>({ label, items, textOf, children, scroll = false }: {
  label: string; items: readonly Item[]; textOf: (item: Item) => string;
  children: (items: readonly Item[]) => ReactNode;
  /** Tables provide their own scroller; plain declaration rows use the same named region here. */
  scroll?: boolean;
}) {
  const { t } = useTranslation();
  const search = useCollectionSearch(label);
  const words = search.text.toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  const scope = JSON.stringify([search.scope, words]);
  const [page, setPage] = useState({ scope, count: PAGE_ROWS });
  const count = page.scope === scope ? page.count : PAGE_ROWS;
  const matches = words.length === 0 ? items : items.filter((item) => {
    const text = textOf(item).toLocaleLowerCase();
    return words.every((word) => text.includes(word));
  });
  const content = matches.length === 0 && search.text.trim() !== '' ? <EmptyState compact title={t('ui.collection.none')} /> : children(matches.slice(0, count));
  return <div className="flex min-w-0 flex-col gap-3">
    {(items.length > 10 || search.text !== '') && <CollectionSearch name={label} {...search} />}
    {scroll ? <ScrollRegion kind="table" label={label} className="max-h-96 overflow-y-auto">{content}</ScrollRegion> : content}
    <Pagination hasMore={matches.length > count} loading={false} onMore={() => setPage({ scope, count: count + PAGE_ROWS })} />
  </div>;
}
