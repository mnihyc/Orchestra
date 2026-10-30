/** Shared Badge component (status); primitives are wrapped here.
 *
 * A count, or a word or two, that marks something as needing a look: how many are unread, how many
 * wait. A count past its ceiling is shown as the ceiling with a plus.
 */
import type { ReactNode } from 'react';

import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { cx } from '@/ui/cx';
import { StatusChip } from '@/ui/StateChip';

export type BadgeTone = 'accent' | 'danger' | 'warning' | 'neutral';

/** The highest count a badge writes out: beyond two digits a badge is no longer read at a glance. */
const COUNT_CEILING = 99;

export interface BadgeProps {
  tone?: BadgeTone;
  /** A number to show; nothing is drawn for zero. */
  count?: number;
  /** Words to show instead of a number. */
  children?: ReactNode;
  /** What the number counts, for a screen reader: "3 unread". */
  label?: string;
  className?: string;
}

const TONES: Record<BadgeTone, string> = {
  accent: 'bg-accent-solid text-text-on-accent',
  danger: 'bg-danger-solid text-text-on-accent',
  warning: 'border border-warning/40 bg-warning-subtle text-text',
  neutral: 'bg-bg-muted text-text-muted',
};

export function Badge({ tone = 'neutral', count, children, label, className }: BadgeProps) {
  const locale = useAppearance((state) => state.locale);
  if (count !== undefined && count <= 0) return null;
  if (count === undefined) return <StatusChip tone={tone} className={className}>
    <span className="inline-flex items-center gap-1" aria-hidden={label === undefined ? undefined : true}>{children}</span>
    {label !== undefined && <span className="sr-only">{label}</span>}
  </StatusChip>;
  const shown =
    count > COUNT_CEILING
        ? `${formatNumber(COUNT_CEILING, locale)}+`
        : formatNumber(count, locale);
  return (
    <span
      className={cx(
        'inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full px-1 text-xs font-medium tabular-nums',
        TONES[tone],
        className,
      )}
    >
      <span aria-hidden={label === undefined ? undefined : true}>{shown}</span>
      {label !== undefined && <span className="sr-only">{label}</span>}
    </span>
  );
}
