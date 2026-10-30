/** Even a large list of losses or warnings draws one page, keeping every reason reachable. */
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ImportPagination } from '@/features/people/ImportPagination';
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
  const [page, setPage] = useState(0);
  const start = Math.min(page, Math.max(0, Math.floor((reasons.length - 1) / REASON_PAGE) * REASON_PAGE));
  if (reasons.length === 0) return null;
  return <div className="flex min-w-0 flex-col gap-2">
    <ul aria-label={label} className="flex flex-col gap-1">{reasons.slice(start, start + REASON_PAGE).map((reason, index) => {
      const count = countOf(reason);
      const words = <>{importReason(reason, t, locale)}{count === null ? '' : ` · ${formatNumber(count, locale)}`}</>;
      return <li key={`${reason.code}:${start + index}`} className="text-sm text-text-muted">{danger ? <InlineAlert tone="danger">{words}</InlineAlert> : words}</li>;
    })}</ul>
    <ImportPagination first={start > 0} more={reasons.length > start + REASON_PAGE} onFirst={() => setPage(0)} onMore={() => setPage(start + REASON_PAGE)} />
  </div>;
}
