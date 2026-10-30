/** Shared Breadcrumbs component (navigation); primitives are wrapped here.
 *
 * The places above a screen, ending at its parent. Names wrap in full, never into an overflow menu
 * or a few clipped letters. Below 600px the nearest parent is the one back link. Redacted places
 * stay named and unlinked.
 */
import { ChevronLeft, ChevronRight } from 'lucide-react';
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface Crumb {
  id: string;
  /** Its name, as the folded list and a screen reader say it. */
  label: string;
  href?: string | undefined;
  /** The thing whose sub-screen is in view; keep it beside that sub-screen on a phone. */
  important?: boolean;
  /** Goes there. Absent for a place that is no link: the screen itself, or one the reader may not open. */
  go?: () => void;
  /** Draws the name as a link. Absent where there is nothing to go to. */
  render?: (className: string) => ReactNode;
  /** A small mark before the name. */
  icon?: ReactNode;
}

export interface BreadcrumbsProps {
  /** The parents, outermost first; the last one is the nearest parent. */
  crumbs: readonly Crumb[];
  /** Retained for callers from older builds; parents are no longer folded. */
  max?: number;
  className?: string;
}

const LINK = 'focus-inset inline-flex min-h-6 min-w-6 max-w-full items-center rounded-sm px-1 whitespace-normal wrap-anywhere text-text-muted transition-colors hover:text-text';

export function Breadcrumbs({ crumbs, className }: BreadcrumbsProps) {
  const { t } = useTranslation();
  if (crumbs.length === 0) return null;
  const last = crumbs.at(-1);

  const draw = (crumb: Crumb) => {
    const current = crumb === last;
    return (
      <li key={crumb.id} className={cx('min-w-0 max-w-full items-center gap-1', current ? 'flex' : 'hidden min-[600px]:flex')}>
        {current && <ChevronLeft aria-hidden className="shrink-0 min-[600px]:hidden" />}
        {crumb !== crumbs[0] && <ChevronRight aria-hidden className="hidden shrink-0 text-text-disabled min-[600px]:block" />}
        {crumb.icon !== undefined && <span className="hidden shrink-0 text-text-subtle min-[600px]:inline-flex">{crumb.icon}</span>}
        {crumb.href !== undefined ? (
          <Tooltip content={crumb.label}>
            <a href={crumb.href} className={LINK} title={crumb.label}
              onClick={(event) => {
                if (crumb.go === undefined || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
                event.preventDefault(); crumb.go();
              }}>{crumb.label}</a>
          </Tooltip>
        ) : crumb.render ? (
          crumb.render(LINK)
        ) : crumb.go !== undefined ? (
          <button type="button" className={LINK} onClick={crumb.go}>{crumb.label}</button>
        ) : (
          <span className="inline-flex min-h-6 items-center whitespace-normal wrap-anywhere text-text-subtle">{crumb.label}</span>
        )}
      </li>
    );
  };

  return (
    <nav data-screen-path aria-label={t('ui.breadcrumbs.label')} className={cx('min-w-0', className)}>
      <ol className="flex min-w-0 flex-wrap items-center gap-x-1 gap-y-0.5 text-base">{crumbs.map(draw)}</ol>
    </nav>
  );
}
