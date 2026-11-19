/** Shared Menu component (overlays); primitives are wrapped here.
 *
 * The list of what can be done with a thing, opened from a button. It is written once, as entries,
 * and the same entries are what the thing's right-click menu shows (`ContextMenu`), so the two can
 * never differ.
 *
 * An entry the reader may not use, and would look for, stays in the list: greyed, reachable by the
 * arrow keys, with the reason under its name. Entries that destroy come last, apart, in the colour
 * of danger. Each entry shows its shortcut.
 */
import { Check, ChevronRight } from 'lucide-react';
import { DropdownMenu } from 'radix-ui';
import { isValidElement, type Ref, type ComponentProps, type ComponentType, type ReactNode } from 'react';

import { ActionIcon, actionForLabel, type ActionId } from '@/ui/actions';
import { Choice, ChoicePicker, choiceName, hasChoiceDescription, type ChoiceDetails, type ChoiceName } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { ChoiceCard } from '@/ui/DetailCard';
import { Kbd } from '@/ui/Kbd';
import { cardClasses, floatingAttachment, floatingClasses, useFloatingSubmenu } from '@/ui/Popover';
import { useScrolls } from '@/ui/useScrolls';
import { useTransientLayer } from '@/ui/PromptScope';
import { isInspectionTarget } from '@/ui/inspection';

export type MenuEntry =
  | (ChoiceName<string> & ChoiceDetails & {
      type?: 'item';
      action?: ActionId;
      /** A real link keeps browser copy-link and open-in-new-tab behavior. */
      href?: string | undefined;
      id: string;
      icon?: ReactNode;
      /** The shortcut that does the same: `Mod+K`. */
      shortcut?: string;
      /** Marks an entry that destroys. */
      danger?: boolean;
      /** Why the reader may not use it; the entry is then shown greyed with these words. */
      disabled?: string | false;
      onSelect: () => void;
    })
  | { type: 'separator'; id: string }
  | { type: 'label'; id: string; label: string }
  | { type: 'sub'; action?: ActionId; id: string; label: string; icon?: ReactNode; entries: readonly MenuEntry[] }
  | {
      type: 'radio';
      id: string;
      /** What is being chosen, for a screen reader. */
      label: string;
      value: string;
      onValueChange: (value: string) => void;
      options: readonly (ChoiceName<string> & ChoiceDetails & { value: string; icon?: ReactNode })[];
    }
  | (ChoiceName<string> & ChoiceDetails & { type: 'check'; action?: ActionId; id: string; icon?: ReactNode; checked: boolean; onCheckedChange: (checked: boolean) => void });

/** The parts a menu is made of. The button's menu and the right-click menu each bring their own. */
export interface MenuParts {
  Item: ComponentType<{
    className?: string;
    children?: ReactNode;
    onSelect?: (event: Event) => void;
    asChild?: boolean;
    'aria-disabled'?: boolean;
    'aria-label'?: string;
    'aria-labelledby'?: string;
    'aria-describedby'?: string;
    textValue?: string;
    title?: string;
  }>;
  Separator: ComponentType<{ className?: string }>;
  Label: ComponentType<{ className?: string; children?: ReactNode }>;
  Sub: ComponentType<{ children?: ReactNode }>;
  SubTrigger: ComponentType<{ className?: string; children?: ReactNode; ref?: Ref<HTMLDivElement> }>;
  SubContent: ComponentType<{ hideWhenDetached?: boolean; ref?: Ref<HTMLDivElement>; className?: string; children?: ReactNode; sideOffset?: number; collisionPadding?: number; onFocusOutside?: ComponentProps<typeof DropdownMenu.SubContent>['onFocusOutside'] }>;
  Portal: ComponentType<{ children?: ReactNode }>;
  RadioGroup: ComponentType<{ value?: string; onValueChange?: (value: string) => void; children?: ReactNode; 'aria-label'?: string }>;
  RadioItem: ComponentType<{ value: string; className?: string; children?: ReactNode; 'aria-labelledby'?: string; 'aria-describedby'?: string; textValue?: string }>;
  CheckboxItem: ComponentType<{
    checked?: boolean;
    onCheckedChange?: (checked: boolean) => void;
    className?: string;
    children?: ReactNode;
    'aria-labelledby'?: string;
    'aria-describedby'?: string;
    textValue?: string;
  }>;
  ItemIndicator: ComponentType<{ className?: string; children?: ReactNode }>;
}

export const menuContentClasses = cx(
  'z-(--layer-dropdown) [--floating-width:var(--menu-max-width)] min-w-[min(var(--menu-min-width),calc(100dvw-var(--spacing-4)))] animate-rise-in overflow-y-auto p-1 text-base text-text',
  floatingClasses,
  cardClasses,
);

const ROW_BASE =
  'focus-inset flex min-h-8 cursor-default gap-2 rounded-sm px-2 py-1 select-none data-[highlighted]:bg-bg-muted';
const ROW = cx(ROW_BASE, 'items-center');

/** A custom row, including a drop target, keeps the menu's keyboard and focus behavior. */
export function MenuItem({ className, ...props }: ComponentProps<typeof DropdownMenu.Item>) {
  return <DropdownMenu.Item {...props} className={cx(ROW, className)} />;
}

/** Draws entries with the parts of one kind of menu. */
export function MenuEntries({ parts, entries }: { parts: MenuParts; entries: readonly MenuEntry[] }) {
  return (
    <>
      {entries.map((entry) => {
        switch (entry.type) {
          case 'separator':
            return <parts.Separator key={entry.id} className="-mx-1 my-1 h-px bg-border" />;
          case 'label':
            return (
              <parts.Label key={entry.id} className="px-2 py-1 text-xs font-medium text-text-subtle">
                {entry.label}
              </parts.Label>
            );
          case 'sub':
            return <SubEntries key={entry.id} parts={parts} entry={entry} />;
          case 'radio':
            return (
              <parts.RadioGroup
                key={entry.id}
                aria-label={entry.label}
                value={entry.value}
                onValueChange={entry.onValueChange}
              >
                {entry.options.map((option) => (
                  <Choice presentation="picker" key={option.value} name={option.preview?.renderName?.(choiceName(option)) ?? choiceName(option)} description={option.description} compact
                  >{(content, control) => <ChoiceCard preview={option.preview} renderInspect={(button) => <parts.Item asChild onSelect={(event) => event.preventDefault()}>{button}</parts.Item>}><parts.RadioItem {...control} value={option.value} textValue={choiceName(option)} className={cx(ROW_BASE, hasChoiceDescription(option.description) ? 'items-start' : 'items-center', 'choice-option min-w-0 flex-1 data-[state=checked]:bg-accent-subtle data-[state=checked]:font-medium')}>
                    <span className={cx('flex size-4 shrink-0 items-center justify-center text-accent-text', hasChoiceDescription(option.description) && 'mt-0.5')}>
                      <parts.ItemIndicator>
                        <Check aria-hidden />
                      </parts.ItemIndicator>
                    </span>
                    {option.icon !== undefined && <span className="text-text-muted" aria-hidden>{option.icon}</span>}
                    {content}
                  </parts.RadioItem></ChoiceCard>}</Choice>
                ))}
              </parts.RadioGroup>
            );
          case 'check':
            return (
              <Choice
                presentation="picker"
                key={entry.id}
                name={choiceName(entry)}
                description={entry.description}
                compact
              >{(content, control) => <parts.CheckboxItem
                {...control}
                checked={entry.checked}
                onCheckedChange={entry.onCheckedChange}
                textValue={choiceName(entry)}
                className={cx(ROW, 'choice-option min-w-0 flex-1')}
              >
                <span className="flex size-4 items-center justify-center text-accent-text">
                  <parts.ItemIndicator>
                    <Check aria-hidden />
                  </parts.ItemIndicator>
                </span>
                {(entry.action !== undefined || entry.icon !== undefined) && <span className="text-text-muted">{entry.action === undefined ? entry.icon : <ActionIcon action={entry.action} />}</span>}
                {content}
              </parts.CheckboxItem>}</Choice>
            );
          default: {
            const off = typeof entry.disabled === 'string';
            const name = choiceName(entry);
            const action = entry.action ?? actionForLabel(name);
            const description = off ? <span className="flex flex-col gap-0.5">
              {entry.description !== undefined && <span>{entry.description}</span>}
              {' '}<span>{entry.disabled}</span>
            </span> : entry.description;
            return (
              <Choice presentation="picker" key={entry.id} name={entry.preview?.renderName?.(name) ?? name} description={description} compact
              >{(content, control) => {
                const row = <>
                  <span aria-hidden className={cx('flex size-4 shrink-0 items-center justify-center', !off && entry.danger !== true && 'text-text-muted')}>
                    {action === undefined ? entry.icon : <ActionIcon action={action} />}
                  </span>
                  {content}
                  {entry.shortcut !== undefined && <Kbd shortcut={entry.shortcut} className="ml-4" />}
                </>;
                return <ChoiceCard preview={entry.preview} renderInspect={(button) => <parts.Item asChild onSelect={(event) => event.preventDefault()}>{button}</parts.Item>}><parts.Item {...control} asChild={entry.href !== undefined && !off} textValue={name}
                {...(off ? { 'aria-disabled': true } : {})}
                onSelect={(event) => { if (off) event.preventDefault(); else if (entry.href === undefined) entry.onSelect(); }}
                className={cx(ROW, 'min-w-0 flex-1', off ? 'text-text-muted' : entry.danger === true ? 'text-danger-text data-[highlighted]:bg-danger-subtle' : 'text-text')}>
                {entry.href !== undefined && !off ? <a href={entry.href}>{row}</a> : row}
              </parts.Item></ChoiceCard>;
              }}</Choice>
            );
          }
        }
      })}
    </>
  );
}

function SubEntries({ parts, entry }: { parts: MenuParts; entry: Extract<MenuEntry, { type: 'sub' }> }) {
  const { trigger, attach, offset } = useFloatingSubmenu();
  return <parts.Sub>
    <parts.SubTrigger ref={trigger} className={cx(ROW, 'data-[state=open]:bg-bg-muted')}>
      <span className="flex size-4 items-center justify-center text-text-muted">{entry.action === undefined ? entry.icon : <ActionIcon action={entry.action} />}</span>
      <span className="flex-1">{entry.label}</span>
      <ChevronRight className="text-text-subtle" aria-hidden />
    </parts.SubTrigger>
    <parts.Portal>
      <parts.SubContent ref={attach} className={menuContentClasses} sideOffset={offset} collisionPadding={8} {...floatingAttachment} onFocusOutside={(event) => {
        // A fast diagonal pointer exit can focus the parent before Radix records its
        // new horizontal direction. Let its grace area take over; another item or
        // any outside target still closes the submenu normally.
        if (event.target === trigger.current?.closest('[role="menu"]')) event.preventDefault();
      }}>
        <MenuEntries parts={parts} entries={entry.entries} />
      </parts.SubContent>
    </parts.Portal>
  </parts.Sub>;
}

const DROPDOWN_PARTS: MenuParts = {
  Item: DropdownMenu.Item,
  Separator: DropdownMenu.Separator,
  Label: DropdownMenu.Label,
  Sub: DropdownMenu.Sub,
  SubTrigger: DropdownMenu.SubTrigger,
  SubContent: DropdownMenu.SubContent,
  Portal: DropdownMenu.Portal,
  RadioGroup: DropdownMenu.RadioGroup,
  RadioItem: DropdownMenu.RadioItem,
  CheckboxItem: DropdownMenu.CheckboxItem,
  ItemIndicator: DropdownMenu.ItemIndicator,
};

/** The entries of a list drawn as the parts of a button's menu: for what is given as `content` and so is made only once the menu is open. */
export function DropdownEntries({ entries }: { entries: readonly MenuEntry[] }) {
  return <MenuEntries parts={DROPDOWN_PARTS} entries={entries} />;
}

export interface MenuProps {
  /** The one button that opens the menu. */
  trigger: ReactNode;
  entries?: readonly MenuEntry[];
  /**
   * What fills the list, in place of `entries`, drawn only while the menu is open. A row of a long
   * list gives its entries this way, so that nothing is made for every row, only for the one asked.
   */
  content?: ReactNode;
  /** What the list is of, for a screen reader. */
  label: string;
  side?: 'top' | 'right' | 'bottom' | 'left';
  align?: 'start' | 'center' | 'end';
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** A destination selected in this menu may take focus instead of returning it to the trigger. */
  onCloseAutoFocus?: ComponentProps<typeof DropdownMenu.Content>['onCloseAutoFocus'];
}

export function Menu({ trigger, entries = [], content, label, side = 'bottom', align = 'start', open, onOpenChange, onCloseAutoFocus }: MenuProps) {
  const [shown, changeOpen, covered] = useTransientLayer(open, onOpenChange);
  const { attach, scrollProps } = useScrolls();
  return (
    // A menu lies over the page without closing the page off: a press beside it puts it away and
    // lands where it was aimed, and nothing under it is hidden from a screen reader meanwhile.
    <DropdownMenu.Root modal={false} open={shown} onOpenChange={changeOpen}>
      <DropdownMenu.Trigger asChild disabled={isValidElement<{ disabled?: boolean }>(trigger) && trigger.props.disabled === true}>{trigger}</DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content
          onCloseAutoFocus={onCloseAutoFocus}
          style={covered ? { display: 'none' } : undefined}
          onInteractOutside={(event) => { if (covered || isInspectionTarget(event.target)) event.preventDefault(); }}
          ref={attach}
          {...scrollProps}
          aria-label={label} aria-labelledby={undefined}
          side={side}
          align={align}
          sideOffset={4}
          collisionPadding={8} {...floatingAttachment}
          className={menuContentClasses}
        >
          <ChoicePicker>{content ?? <MenuEntries parts={DROPDOWN_PARTS} entries={entries} />}</ChoicePicker>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}
