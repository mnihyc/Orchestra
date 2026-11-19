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
import { Dialog as Primitive } from 'radix-ui';
import type { ComponentProps, KeyboardEvent, ReactNode } from 'react';

import { cx } from '@/ui/cx';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { overlayClasses, useOpener } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { PromptScope, usePrompt } from '@/ui/PromptScope';

export interface PaletteProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** What it is, for a screen reader. It is not drawn. */
  label: string;
  /** A focused chooser names its list visibly, without a generic result-group heading. */
  showTitle?: boolean;
  /** The field's own accessible name, distinct from the dialog's title where supplied. */
  inputLabel?: string;
  /** The row that is chosen now: the one Enter would take. */
  value: string;
  onValueChange: (value: string) => void;
  /** Sees a key before the list does; one that calls `preventDefault` is not taken by the list. */
  onKeyDown?: (event: KeyboardEvent<HTMLDivElement>) => void;
  /** A selected destination may take focus instead of returning it to the opening button. */
  onCloseAutoFocus?: ComponentProps<typeof Primitive.Content>['onCloseAutoFocus'];
  children: ReactNode;
}

export function Palette({ open, onOpenChange, label, inputLabel = label, showTitle = false, value, onValueChange, onKeyDown, onCloseAutoFocus, children }: PaletteProps) {
  const layer = usePrompt(open);
  const opener = useOpener();
  return (
    <PromptScope layer={layer}><Primitive.Root open={open} onOpenChange={onOpenChange}>
      <Primitive.Portal>
        <Primitive.Overlay style={layer.style} className={overlayClasses} />
        <div style={layer.style} className="pointer-events-none fixed inset-0 z-(--layer-modal) flex items-start justify-center p-4 pt-(--palette-top)">
          <Primitive.Content
            data-prompt-id={layer.id}
            data-prompt-current={!layer.covered}
            aria-describedby={undefined}
            onCloseAutoFocus={(event) => { onCloseAutoFocus?.(event); if (!event.defaultPrevented) opener.giveBack(event); }}
            onOpenAutoFocus={(event) => {
              opener.remember();
              const input = (event.currentTarget as HTMLElement).querySelector<HTMLElement>('input');
              if (input === null) return;
              event.preventDefault();
              input.focus();
            }}
            className="pointer-events-auto flex max-h-(--dialog-max-height) w-full max-w-dialog-lg flex-col overflow-hidden rounded-xl border border-border bg-surface-raised shadow-lg data-[state=open]:animate-rise-in"
          >
            <Primitive.Title className={showTitle ? 'px-4 pt-3 text-base font-semibold' : 'sr-only'}>{label}</Primitive.Title>
            <ContentBoundary resetKey={open} className="m-3">
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
            </ContentBoundary>
          </Primitive.Content>
        </div>
      </Primitive.Portal>
    </Primitive.Root></PromptScope>
  );
}

/** The field, with a magnifier before it and anything the caller wants after it. */
export function PaletteInput({ trailing, className, ...input }: ComponentProps<typeof Command.Input> & { trailing?: ReactNode }) {
  return (
    <div className="flex items-center gap-2 border-b border-border px-4">
      <Search className="text-text-subtle" aria-hidden />
      <Command.Input
        className={cx(
          'focus-inset my-2 h-(--control-md) min-w-0 flex-1 rounded-sm border border-control-border bg-surface px-2 text-md text-text placeholder:text-text-subtle',
          className,
        )}
        {...input}
      />
      {trailing}
    </div>
  );
}

/** The scrolling list of rows. */
export function PaletteList({ className, ...list }: ComponentProps<typeof Command.List>) {
  return <Command.List className={cx('max-h-(--palette-list-height) overflow-y-auto overscroll-contain p-2', className)} {...list} />;
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
      className={cx(
        'flex min-h-10 cursor-default items-center gap-3 rounded-md px-2 py-1 text-base text-text select-none',
        'data-[selected=true]:bg-accent-subtle data-[selected=true]:outline-1 data-[selected=true]:-outline-offset-1 data-[selected=true]:outline-accent data-[disabled=true]:text-text-muted',
        className,
      )}
      {...item}
    />
  );
}

/** What stands in the list when nothing is in it. */
export function PaletteEmpty({ className, children, ...empty }: ComponentProps<typeof Command.Empty>) {
  return <Command.Empty className={cx('px-4', className)} {...empty}><EmptyState compact title={children} /></Command.Empty>;
}
