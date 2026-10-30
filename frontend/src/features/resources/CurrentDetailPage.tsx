/** Read-only detail lists keep only their current page mounted. */
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { DETAIL_PAGE_ROWS } from '@/features/resources/detailPageSize';
import { formatNumber } from '@/lib/format';
import { Button } from '@/ui/Button';

/** Editors keep their selections above this window so paging never loses a choice. */
export function CurrentDetailPage({ count, children, blocked = false }: {
  count: number; children: (from: number, to: number) => ReactNode; blocked?: boolean;
}) {
  const { t, i18n } = useTranslation();
  const [page, setPage] = useState(0);
  const pages = Math.max(1, Math.ceil(count / DETAIL_PAGE_ROWS));
  const current = Math.min(page, pages - 1);
  if (current !== page) setPage(current);
  const from = current * DETAIL_PAGE_ROWS, to = Math.min(count, from + DETAIL_PAGE_ROWS);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  return <div className="flex min-w-0 flex-col gap-3">
    {children(from, to)}
    {pages > 1 && <div className="flex min-w-0 flex-wrap items-center justify-between gap-2">
      <span role="status" className="text-sm text-text-muted">{t('programList.pageRange', { from: formatNumber(from + 1, locale), to: formatNumber(to, locale), total: formatNumber(count, locale) })}</span>
      <div className="flex gap-2"><Button size="sm" disabled={blocked || current === 0} onClick={() => setPage(current - 1)}>{t('resourceDetails.layout.previous')}</Button>
        <Button size="sm" disabled={blocked || current === pages - 1} onClick={() => setPage(current + 1)}>{t('resourceDetails.layout.next')}</Button></div>
    </div>}
  </div>;
}
