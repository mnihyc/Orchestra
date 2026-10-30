/** One wrapping row for search, participation, filters and the page's primary action. */
import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';

/** The common compact height also applies to ordinary buttons/fields placed beside a toolbar select. */
// Segmented tracks keep their two-pixel inset; their items use the remaining compact control height.
export const compactControls = '[--control-md:var(--control-sm)] [&_input]:px-2 [&_input]:text-sm'
  + ' [&_[data-segmented]]:p-0.5'
  + ' [&_[data-segmented]_.choice-option]:min-h-[calc(var(--control-sm)-4px)]'
  + ' [&_[data-segmented]_.choice-option]:py-0 [&_[data-segmented]_.choice-option]:text-sm';

export interface FilterBarProps {
  label?: string;
  search?: ReactNode;
  /** A page-wide view choice precedes search and the narrower criteria. */
  leading?: ReactNode;
  participation?: ReactNode;
  children?: ReactNode;
  /** Compact popover-based toolbars can keep search and their short triggers on one line. */
  wrap?: boolean;
  /** An optional filter action at the right. Resource creation belongs in PageHeader. */
  action?: ReactNode;
  className?: string;
}

export function FilterBar({ label, search, leading, participation, children, action, wrap = true, className }: FilterBarProps) {
  const { t } = useTranslation();
  return <div role="group" aria-label={label ?? t('ui.filterBar.label')} data-filter-bar
    className={cx('flex min-w-0 items-end', wrap ? 'flex-wrap gap-x-3 gap-y-3' : 'flex-nowrap gap-2', compactControls, className)}>
    {leading}
    {search !== undefined && <div data-filter-search className={wrap ? 'min-w-0 flex-1 basis-[min(100%,16rem)] sm:max-w-(--container-form)' : 'min-w-0 flex-1 basis-0'}>{search}</div>}
    {participation !== undefined && <div className="flex min-h-(--control-sm) max-w-full items-center">{participation}</div>}
    {children}
    {action !== undefined && action !== null && <div data-filter-action className="ml-auto flex max-w-full flex-wrap items-center justify-end gap-2">{action}</div>}
  </div>;
}
