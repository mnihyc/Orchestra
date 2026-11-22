/** Shared Palette component (overlays); primitives are wrapped here.
 *
 * One field with a list under it, opened over the page: what the finder is. The field narrows the
 * list, the arrow keys move along it and Enter chooses; the list is made by whoever draws the palette,
 * which also decides what narrows it, so nothing here filters or sorts. The library underneath
 * (`cmdk`) keeps the roles that make the field a combobox and its rows options, and the focus in the
 * field while the rows are walked.
 */
import { Command } from 'cmdk';
import { Search } from 'lucide-react';
import type { ComponentProps, KeyboardEvent, ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { PaletteDialog, type PaletteDialogProps } from '@/ui/PaletteDialog';
import { inputClasses, itemClasses, listClasses } from '@/ui/paletteStyle';

export interface PaletteProps extends PaletteDialogProps {
  /** The field's own accessible name, distinct from the dialog's title where supplied. */
  inputLabel?: string;
  /** The row that is chosen now: the one Enter would take. */
  value: string;
  onValueChange: (value: string) => void;
  /** Sees a key before the list does; one that calls `preventDefault` is not taken by the list. */
  onKeyDown?: (event: KeyboardEvent<HTMLDivElement>) => void;
}

export function Palette({ label, inputLabel = label, value, onValueChange, onKeyDown, children, ...dialog }: PaletteProps) {
  return (
    <PaletteDialog {...dialog} label={label}>
      <Command
        label={inputLabel}
        shouldFilter={false}
        loop
        // Control+J and Control+K are the workbench's, not a way to move along a list.
        vimBindings={false}
        value={value}
        onValueChange={onValueChange}
        {...(onKeyDown === undefined ? {} : { onKeyDown })}
        className="flex min-h-0 flex-col"
      >
        {children}
      </Command>
    </PaletteDialog>
  );
}

/** The field, with a magnifier before it and anything the caller wants after it. */
export function PaletteInput({ trailing, className, ...input }: ComponentProps<typeof Command.Input> & { trailing?: ReactNode }) {
  return (
    <div className="flex items-center gap-2 border-b border-border px-4">
      <Search className="text-text-subtle" aria-hidden />
      <Command.Input
        className={cx(inputClasses, className)}
        {...input}
      />
      {trailing}
    </div>
  );
}

/** The scrolling list of rows. */
export function PaletteList({ className, ...list }: ComponentProps<typeof Command.List>) {
  return <Command.List className={cx(listClasses, className)} {...list} />;
}

/** A heading and the rows under it. */
export function PaletteGroup({ className, ...group }: ComponentProps<typeof Command.Group>) {
  return (
    <Command.Group
      className={cx(
        '[&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:pt-3 [&_[cmdk-group-heading]]:pb-1 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-text-muted',
        className,
      )}
      {...group}
    />
  );
}

/** One row. The chosen row is shaded; one that cannot be chosen is dimmed and the keys pass over it. */
export function PaletteItem({ className, ...item }: ComponentProps<typeof Command.Item>) {
  return (
    <Command.Item
      className={cx(itemClasses, className)}
      {...item}
    />
  );
}

/** What stands in the list when nothing is in it. */
export function PaletteEmpty({ className, children, ...empty }: ComponentProps<typeof Command.Empty>) {
  return <Command.Empty className={cx('px-4', className)} {...empty}><EmptyState compact title={children} /></Command.Empty>;
}
