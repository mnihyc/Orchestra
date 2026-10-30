/** Shared ContextMenu component (overlays): the same list as a thing's "⋯", opened by a right click, the menu key or a long press, with entries one may not use disabled with the reason.
 *
 * It takes the entries a `Menu` takes and draws them the same way. A right click opens it, and so do
 * the menu key and `Shift+F10` for whatever has the focus, and a long press on a touch screen.
 */
import { ContextMenu as Primitive } from 'radix-ui';
import type { ReactNode } from 'react';
import { ChoicePicker } from '@/ui/Choice';

import { menuContentClasses, MenuEntries, type MenuEntry, type MenuParts } from '@/ui/Menu';
import { useScrolls } from '@/ui/useScrolls';
import { usePromptCovered } from '@/ui/PromptScope';
import { isInspectionTarget } from '@/ui/inspection';

const CONTEXT_PARTS: MenuParts = {
  Item: Primitive.Item,
  Separator: Primitive.Separator,
  Label: Primitive.Label,
  Sub: Primitive.Sub,
  SubTrigger: Primitive.SubTrigger,
  SubContent: Primitive.SubContent,
  Portal: Primitive.Portal,
  RadioGroup: Primitive.RadioGroup,
  RadioItem: Primitive.RadioItem,
  CheckboxItem: Primitive.CheckboxItem,
  ItemIndicator: Primitive.ItemIndicator,
};

/** The entries of a list drawn as the parts of a right-click menu: for what is given as `content` and so is made only once the menu is open. */
export function ContextEntries({ entries }: { entries: readonly MenuEntry[] }) {
  return <MenuEntries parts={CONTEXT_PARTS} entries={entries} />;
}

export interface ContextMenuProps {
  entries?: readonly MenuEntry[];
  /** What fills the list, in place of `entries`, drawn only while the menu is open: nothing is made for a row until it is asked. */
  content?: ReactNode;
  /** What the list is of, for a screen reader. */
  label: string;
  /** The one element whose menu this is. */
  children: ReactNode;
  /** Leaves the browser's own menu alone, where there is nothing to offer. */
  disabled?: boolean;
  /** Told when the menu opens and closes, so that what its entries depend on is asked for only when it is wanted. */
  onOpenChange?: (open: boolean) => void;
}

export function ContextMenu({ entries = [], content, label, children, disabled = false, onOpenChange }: ContextMenuProps) {
  const covered = usePromptCovered();
  const { attach, scrollProps } = useScrolls();
  return (
    <Primitive.Root modal={false} {...(onOpenChange ? { onOpenChange } : {})}>
      <Primitive.Trigger asChild disabled={disabled || (content === undefined && entries.length === 0)} onKeyDown={(event) => {
        if (event.defaultPrevented) return;
        if (event.key !== 'ContextMenu' && !(event.shiftKey && event.key === 'F10')) return;
        if (disabled || (content === undefined && entries.length === 0)) return;
        event.preventDefault();
        const bounds = event.currentTarget.getBoundingClientRect();
        event.currentTarget.dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: bounds.left, clientY: bounds.bottom }));
      }}>
        {children}
      </Primitive.Trigger>
      <Primitive.Portal>
        <Primitive.Content style={covered ? { display: 'none' } : undefined} ref={attach} {...scrollProps} aria-label={label} collisionPadding={8} className={menuContentClasses}
          onInteractOutside={(event) => { if (isInspectionTarget(event.target)) event.preventDefault(); }}>
          <ChoicePicker>{content ?? <MenuEntries parts={CONTEXT_PARTS} entries={entries} />}</ChoicePicker>
        </Primitive.Content>
      </Primitive.Portal>
    </Primitive.Root>
  );
}
