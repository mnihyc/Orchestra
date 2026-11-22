/** Even a large list of losses or warnings draws one page, keeping every reason reachable. */
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ImportPagination } from '@/features/people/ImportPagination';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { StateLine } from '@/ui/StateLine';
import { importReason, type ImportReason } from '@/features/people/importReason';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { InlineAlert } from '@/ui/InlineAlert';

const REASON_PAGE = 50;

function countOf(reason: ImportReason & { count?: number }): number | null {
  const params = reason.params;
  const count = reason.count ?? (typeof params === 'object' && params !== null && 'count' in params ? params.count : null);
  return typeof count === 'number' && Number.isFinite(count) && count > 0 ? count : null;
}

export function ImportReasons({ reasons, label, danger = false }: {
  reasons: readonly (ImportReason & { count?: number })[]; label: string; danger?: boolean;
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const search = useCollectionSearch('import-reasons:' + label);
  const scope = search.scope + '\u0000' + search.text;
  const [page, setPage] = useState({ scope, start: 0 });
  const found = useMemo(() => {
    const words = search.text.toLocaleLowerCase().split(/\s+/u).filter(Boolean);
    return words.length === 0 ? reasons : reasons.filter((reason) => {
      const text = importReason(reason, t, locale).toLocaleLowerCase();
      return words.every((word) => text.includes(word));
    });
  }, [reasons, search.text, t, locale]);
  const start = page.scope === scope ? Math.min(page.start, Math.max(0, Math.floor((found.length - 1) / REASON_PAGE) * REASON_PAGE)) : 0;
  const choose = (start: number) => setPage({ scope, start });
  if (reasons.length === 0) return null;
  return <div className="flex min-w-0 flex-col gap-2">
    {(reasons.length > 10 || search.text !== '') && <CollectionSearch name={label} {...search} />}
    {found.length === 0 && <StateLine compact message={t('ui.collection.none')} />}
    <ScrollRegion kind="table" label={label} className="max-h-96 overflow-y-auto"><ul aria-label={label} className="flex flex-col gap-1">{found.slice(start, start + REASON_PAGE).map((reason, index) => {
      const count = countOf(reason);
      const words = <>{importReason(reason, t, locale)}{count === null ? '' : ` · ${formatNumber(count, locale)}`}</>;
      return <li key={`${reason.code}:${start + index}`} className="text-sm text-text-muted">{danger ? <InlineAlert tone="danger">{words}</InlineAlert> : words}</li>;
    })}</ul></ScrollRegion>
    <ImportPagination first={start > 0} more={found.length > start + REASON_PAGE} onFirst={() => choose(0)} onMore={() => choose(start + REASON_PAGE)} />
  </div>;
}
