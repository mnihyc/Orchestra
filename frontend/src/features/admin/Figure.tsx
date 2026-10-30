/** The same locally responsive summary figures serve health and storage, including large values. */
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { boxClasses } from '@/shell/Page';
import { cx } from '@/ui/cx';

export const FIGURE_GRID = 'grid gap-3 @xs:grid-cols-2 @sm:grid-cols-3 @xl:grid-cols-5';

export function Figure({ label, value, hint }: { label: string; value: string | number; hint?: string }) {
  const locale = useAppearance((state) => state.locale);
  return <li className={cx(boxClasses, 'flex min-w-0 flex-col gap-1 p-3')}>
    <span className="min-w-0 truncate text-2xl font-semibold text-text">{typeof value === 'number' ? formatNumber(value, locale) : value}</span>
    <span className="text-sm text-text-muted">{label}</span>
    {hint !== undefined && <span className="text-sm text-text-subtle">{hint}</span>}
  </li>;
}
