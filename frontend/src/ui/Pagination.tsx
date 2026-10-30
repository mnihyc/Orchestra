/** Shared Pagination component (navigation); primitives are wrapped here.
 *
 * The foot of a list that is read a page at a time: "Show more" while there is more, and where the
 * next page could not be had, what happened and a way to ask again. The pages already shown stay
 * where they are, as they were, whatever happens to the one after.
 */
import { RefreshCw } from 'lucide-react';
import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { ErrorAlert } from '@/ui/ErrorState';

export interface PaginationProps {
  /** Whether the server has said there is another page. */
  hasMore: boolean;
  /** The next page is being asked for. */
  loading: boolean;
  /** What the last attempt at the next page failed with, while nothing newer has succeeded. */
  error?: unknown;
  onMore: () => void;
  /**
   * Also asks for the next page when the foot scrolls into view inside whatever scrolls it. The button stays, for the
   * keyboard and for a page that failed: a failed page is asked for again only by the button.
   */
  autoLoad?: boolean;
  className?: string;
}

/** How far before the foot reaches the edge of its scroller the next page is asked for, in pixels. */
const AUTO_LOAD_MARGIN = 120;

export function Pagination({ hasMore, loading, error, onMore, autoLoad = false, className }: PaginationProps) {
  const { t } = useTranslation();
  const failed = error !== undefined && error !== null;
  const foot = useRef<HTMLDivElement>(null);
  const latest = useRef(onMore);
  useEffect(() => { latest.current = onMore; }, [onMore]);
  useEffect(() => {
    const element = foot.current;
    if (!autoLoad || !hasMore || loading || failed || element === null || typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver((entries) => { if (entries.some((entry) => entry.isIntersecting)) latest.current(); }, { rootMargin: `0px 0px ${AUTO_LOAD_MARGIN}px 0px` });
    observer.observe(element);
    return () => observer.disconnect();
  }, [autoLoad, hasMore, loading, failed]);
  if (!hasMore && !failed) return null;
  return (
    <div ref={foot} className={cx('flex flex-col items-center gap-2 py-3', className)}>
      {failed ? <ErrorAlert error={error} className="w-full" action={<Button size="sm" icon={<RefreshCw aria-hidden />} pending={loading} onClick={onMore}>{t('common.tryAgain')}</Button>} /> : <Button
        size="sm"
        pending={loading}
        onClick={onMore}
      >
        {t('ui.pagination.more')}
      </Button>}
    </div>
  );
}
