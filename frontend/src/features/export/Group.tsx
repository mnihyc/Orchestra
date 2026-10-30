/** One labelled row of an export dialog: the name of what is chosen at the start of the line, the choice beside it.
 *
 * The dialogs of this feature are lists of such rows (what, include, format, content and dates), and read the same way:
 * the label on the left, where there is room for one, and above the choice on a narrow screen. The label stands
 * level with the first line of the choice: with the middle of a control of one line (the form), with the first
 * row of a box of rows or cards (the tree, the levels), or with plain text (a tick, a field that carries its own label).
 */
import { type ReactNode, useId } from 'react';

import { cx } from '@/ui/cx';

export interface GroupProps {
  label: ReactNode;
  /** A hint beside the label, for a choice that is not plain. */
  help?: ReactNode;
  /** What the choice begins with, for the label to stand level with: a control of one line (the default), a box of rows or cards, or a line of text. */
  lead?: 'control' | 'box' | 'text';
  children: ReactNode;
  className?: string;
}

/** How far the label is set down: a button is 36 pixels high and a row of a box lies 4 pixels inside it, and a line of text is 20. */
const SET_DOWN = { control: 'pt-2', box: 'pt-3', text: '' } as const;

export function Group({ label, help, lead = 'control', children, className }: GroupProps) {
  const id = useId();
  return (
    <div role="group" aria-labelledby={id} className={cx('grid gap-x-4 gap-y-1 sm:grid-cols-[6.5rem_minmax(0,1fr)]', className)}>
      <span id={id} className={cx('flex items-start gap-1 text-sm font-medium text-text', SET_DOWN[lead])}>
        {label}
        {help !== undefined && <span className="flex shrink-0 items-center">{help}</span>}
      </span>
      <div className="min-w-0">{children}</div>
    </div>
  );
}
