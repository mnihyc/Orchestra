/** Shared RelativeTime component (utilities); primitives are wrapped here.
 *
 * A moment as its reader thinks of it: "3 min. ago" while it is under a day old, a date and a time
 * of day after that, in their own time zone. The exact moment is what the pointer reads on it.
 */
import { useAppearance } from '@/lib/appearance';
import { formatCompactTime, formatExact, formatRelative, useNow } from '@/lib/time';
import { Tooltip } from '@/ui/Tooltip';

export interface RelativeTimeProps {
  /** The moment, as the API writes it. */
  at: string;
  className?: string;
  /** Calendar-aware row form that never needs an ellipsis. */
  compact?: boolean;
}

export function RelativeTime({ at, className, compact = false }: RelativeTimeProps) {
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  const now = useNow();
  const context = { locale, timeZone };
  return (
    <Tooltip content={formatExact(at, context)}>
      <time dateTime={at} className={compact ? `shrink-0 whitespace-nowrap tabular-nums ${className ?? ''}` : className}>
        {compact ? formatCompactTime(at, context, now) : formatRelative(at, context, now)}
      </time>
    </Tooltip>
  );
}
