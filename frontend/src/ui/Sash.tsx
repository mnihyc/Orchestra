/** Shared Sash component (workbench): the border between two areas or groups, dragged or moved with the arrow keys.
 *
 * A line one pixel wide that is easy to take hold of: the pointer catches it from a few pixels
 * either side, and it lights up in the accent's colour while it is held or has the keyboard. With
 * the keyboard on it the arrow keys move it, and Home and End take it to its ends.
 */
import { Separator } from 'react-resizable-panels';

import { cx } from '@/ui/cx';

export interface SashProps {
  /** `vertical`: a line between two things side by side. `horizontal`: between one above another. */
  line: 'vertical' | 'horizontal';
  /** What it moves, for a screen reader: "Width of the left bar". */
  label: string;
  /** A border that is drawn and cannot be moved: one of its sides holds nothing. */
  disabled?: boolean;
  id?: string;
}

export function Sash({ line, label, disabled = false, id }: SashProps) {
  return (
    <Separator
      id={id}
      aria-label={label}
      disabled={disabled}
      className={cx(
        'relative z-(--layer-sticky) shrink-0 bg-border outline-none transition-colors',
        line === 'vertical' ? 'w-px' : 'h-px',
        !disabled &&
          'data-[separator=active]:bg-accent data-[separator=hover]:bg-accent focus-visible:bg-focus focus-visible:ring-2 focus-visible:ring-focus',
      )}
    />
  );
}
