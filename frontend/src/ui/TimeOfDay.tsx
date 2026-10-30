/** A moment as its time of day, with the exact moment under the pointer.
 *
 * A message or a line of history says when it was made as "10:42", or with its date once it is not
 * of today; the exact moment, to the second and with its zone, is what the pointer reads.
 */
import { useFormatContext } from '@/lib/appearance';
import { formatExact, formatTimeOfDay } from '@/lib/time';
import { Tooltip } from '@/ui/Tooltip';

export function TimeOfDay({ at, className }: { at: string; className?: string }) {
  const context = useFormatContext();
  return (
    <Tooltip content={formatExact(at, context)}>
      <time dateTime={at} className={className}>
        {formatTimeOfDay(at, context)}
      </time>
    </Tooltip>
  );
}
