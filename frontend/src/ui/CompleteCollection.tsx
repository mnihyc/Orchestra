/** Complete native declarations and parent-owned plan operands stay whole while their rows are found and paged. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { EmptyState } from '@/ui/EmptyState';
import { Button } from '@/ui/Button';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { ScrollRegion } from '@/ui/ScrollRegion';

/** Match an ordinary server batch without turning that batch into a total limit. */
const PAGE_ROWS = 50;

export function CompleteCollection<Item>({ label, items, textOf, children, scroll = false, pageRows = PAGE_ROWS, pinned = [] }: {
  label: string; items: readonly Item[]; pinned?: readonly Item[]; textOf: (item: Item) => string;
  children: (items: readonly Item[]) => ReactNode;
  /** Tables provide their own scroller; plain declaration rows use the same named region here. */
  scroll?: boolean;
  /** Comfortable forms may keep their established smaller presentation batch. */
  pageRows?: number;
}) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const search = useCollectionSearch(label);
  const words = search.text.toLocaleLowerCase().split(/\s+/u).filter(Boolean);
  const scope = JSON.stringify([search.scope, words]);
  const [page, setPage] = useState({ scope, index: 0 });
  if (page.scope !== scope) setPage({ scope, index: 0 });
  const matches = words.length === 0 ? items : items.filter((item) => {
    const text = textOf(item).toLocaleLowerCase();
    return words.every((word) => text.includes(word));
  });
  const pages = Math.max(1, Math.ceil(matches.length / pageRows));
  const index = page.scope === scope ? Math.min(page.index, pages - 1) : 0;
  const from = index * pageRows;
  const visible = matches.slice(from, from + pageRows);
  const kept = pinned.filter((item) => items.includes(item) && !visible.includes(item));
  const content = <>{matches.length === 0 && search.text.trim() !== '' && <EmptyState compact title={t('ui.collection.none')} />}{(matches.length > 0 || kept.length > 0 || search.text.trim() === '') && children([...kept, ...visible])}</>;
  return <div className="flex min-w-0 flex-col gap-3">
    {(items.length > 10 || search.text !== '') && <CollectionSearch name={label} {...search} />}
    {scroll ? <ScrollRegion kind="table" label={t('ui.collection.find', { name: label })} className="max-h-96 overflow-y-auto">{content}</ScrollRegion> : content}
    {pages > 1 && <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <span role="status" className="text-sm text-text-muted">{t('resourceDetails.layout.range', { from: formatNumber(from + 1, locale), to: formatNumber(Math.min(matches.length, from + pageRows), locale), count: formatNumber(matches.length, locale) })}</span>
      <div className="flex gap-2"><Button size="sm" disabled={index === 0} onClick={() => setPage({ scope, index: index - 1 })}>{t('resourceDetails.layout.previous')}</Button><Button size="sm" disabled={index === pages - 1} onClick={() => setPage({ scope, index: index + 1 })}>{t('resourceDetails.layout.next')}</Button></div>
    </div>}
  </div>;
}
