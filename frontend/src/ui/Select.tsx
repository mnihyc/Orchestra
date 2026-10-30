/** Shared Select component (forms); primitives are wrapped here.
 *
 * One choice among several that are not all in view, written in the same field as a line of text. The
 * list opens under the field, the arrow keys and a typed letter move through it, and the chosen one is
 * marked. For a few things that fit side by side, use a `SegmentedControl`.
 */
import { Check, ChevronDown } from 'lucide-react';
import { Select as Primitive } from 'radix-ui';
import { type ComponentProps, type ReactNode, type SyntheticEvent, useRef, useState } from 'react';

import { Choice, type ChoiceDetails, type ChoiceName } from '@/ui/Choice';
import { ChoiceCard, ChoiceInspection } from '@/ui/DetailCard';
import { cx } from '@/ui/cx';
import { Field, type FieldSlots } from '@/ui/Field';
import { fieldBoxClasses } from '@/ui/Input';
import { cardClasses, floatingAttachment, floatingClasses } from '@/ui/Popover';
import { useTransientLayer } from '@/ui/PromptScope';
import { isInspectionTarget } from '@/ui/inspection';

export type SelectOption<Value extends string> = ChoiceName & ChoiceDetails & {
  /** What the choice is; never empty. */
  value: Value;
  /** The complete original when a caller deliberately shortens an option label before rendering. */
  fullLabel?: string;
  disabled?: boolean;
  /** Separate an administrative view from the ordinary nested views. */
  separatorBefore?: boolean;
};

export interface SelectProps<Value extends string> extends FieldSlots {
  value: Value;
  onValueChange: (value: Value) => void;
  options: readonly SelectOption<Value>[];
  /** What the field says while nothing is chosen. */
  placeholder?: string;
  /** The selected-value text when a toolbar also needs its visible label. */
  valueLabel?: ReactNode;
  /** `sm` for a toolbar, where the label is kept for screen readers. */
  size?: 'sm' | 'md';
  disabled?: boolean;
  /** A caller can keep the current choices visible after a refused write. */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** A navigation picker can focus its destination instead of restoring the trigger. */
  onCloseAutoFocus?: ComponentProps<typeof Primitive.Content>['onCloseAutoFocus'];
  id?: string;
  /** Classes of the frame round label, field and messages. */
  fieldClassName?: string;
  className?: string;
}

export function Select<Value extends string>({
  value,
  onValueChange,
  options,
  placeholder,
  valueLabel,
  size = 'md',
  disabled = false,
  open,
  onOpenChange,
  onCloseAutoFocus,
  label,
  optional,
  hint,
  error,
  hideLabel,
  aside,
  help,
  id,
  fieldClassName,
  className,
}: SelectProps<Value>) {
  const [shown, setShown, covered] = useTransientLayer(open, onOpenChange);
  const [highlighted, setHighlighted] = useState(value);
  const lastItem = useRef<HTMLElement | null>(null);
  const hasGuides = options.some((option) => option.help !== undefined);
  const hasControls = hasGuides || options.some((option) => option.accessory !== undefined || option.preview !== undefined);
  const selected = options.find((option) => option.value === value);
  const active = options.find((option) => option.value === highlighted) ?? selected;
  const activeHelp = active?.help;
  // A disabled value cannot take focus, but its explanation must remain reachable before choosing.
  const guides = options.flatMap((option) => {
    const help = option.help;
    return help !== undefined && (option.value === active?.value || option.disabled === true) ? [{ option, help }] : [];
  });
  const changeOpen = (next: boolean) => {
    if (next) setHighlighted(value);
    setShown(next);
  };
  const openGuide = () => {
    if (activeHelp === undefined) return;
    changeOpen(false);
    activeHelp.onOpen();
  };
  const keepNativeSelection = (event: SyntheticEvent<HTMLDivElement>) => {
    const mirror = event.target;
    // Radix mirrors a controlled value into its hidden native select before a newly added
    // option may have registered there. Setting a missing option produces an empty change;
    // that incomplete mirror must not clear the caller's selection. Real option changes
    // and the form's reset event still follow the primitive's normal handlers.
    if (mirror instanceof HTMLSelectElement && mirror.getAttribute('aria-hidden') === 'true'
      && selected !== undefined && mirror.value !== value
      && !Array.from(mirror.options).some((option) => option.value === value)) event.stopPropagation();
  };
  return (
    <Field label={label} optional={optional} hint={hint} error={error} hideLabel={hideLabel} aside={aside} help={help} className={fieldClassName} id={id}>
      {(control) => (
        <div className="contents" onChangeCapture={keepNativeSelection}>
        <Primitive.Root value={value} onValueChange={(next) => onValueChange(next as Value)} disabled={disabled} open={shown} onOpenChange={changeOpen}>
          <Primitive.Trigger
            {...control}
            aria-haspopup={hasControls ? 'dialog' : 'listbox'}
            className={cx(
              fieldBoxClasses,
              'flex cursor-default items-center justify-between gap-2 text-left',
              size === 'sm' && 'h-(--control-sm) px-2 text-sm',
              className,
            )}
          >
            <span className="flex min-w-0 items-center gap-2" data-overflow-text={selected?.fullLabel}>
              <span className="min-w-0 truncate">
              <Primitive.Value placeholder={placeholder}>{valueLabel}</Primitive.Value>
              </span>
              {selected?.preview?.hint !== undefined && <span className="min-w-0 truncate">{selected.preview.hint}</span>}
            </span>
            <Primitive.Icon className="text-text-subtle">
              <ChevronDown aria-hidden />
            </Primitive.Icon>
          </Primitive.Trigger>
          <Primitive.Portal>
            <Primitive.Content
              style={covered ? { display: 'none' } : undefined}
              onCloseAutoFocus={onCloseAutoFocus}
              onPointerDownOutside={(event) => { if (isInspectionTarget(event.target)) event.preventDefault(); }}
              role={hasControls ? 'dialog' : 'listbox'}
              aria-modal={hasControls ? true : undefined}
              aria-labelledby={hasControls ? control['aria-labelledby'] : undefined}
              position="popper"
              sideOffset={4}
              collisionPadding={8} {...floatingAttachment}
              className={cx(
                'relative z-(--layer-dropdown) [--floating-width:var(--menu-max-width)] min-w-[min(var(--radix-select-trigger-width),var(--menu-max-width),calc(100dvw-var(--spacing-4)))] animate-rise-in overflow-hidden text-base text-text',
                floatingClasses,
                cardClasses,
              )}
              onKeyDown={(event) => {
                if (!hasControls) return;
                if (event.key === 'F1' && activeHelp !== undefined) { event.preventDefault(); openGuide(); }
                if (event.key === 'Tab') {
                  const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[data-choice-help], [data-choice-inspect], [data-choice-accessory] button'));
                  if (buttons.length === 0) return;
                  const current = buttons.findIndex((button) => button === event.target);
                  const next = current < 0 ? event.shiftKey ? buttons.at(-1) : buttons[0] : buttons[current + (event.shiftKey ? -1 : 1)];
                  event.preventDefault();
                  if (next === undefined) lastItem.current?.focus();
                  else next.focus();
                }
              }}
            >
              <ChoiceInspection><Primitive.Viewport role={hasControls ? 'listbox' : 'presentation'} aria-labelledby={hasControls ? control['aria-labelledby'] : undefined} className="p-1">
                {options.map((option) => (
                  <div key={option.value} className="contents">{option.separatorBefore === true && <Primitive.Separator role="separator" className="my-1 h-px bg-border" />}<Choice
                    presentation="picker"
                    key={option.value}
                    name={<Primitive.ItemText><span data-overflow-text={option.fullLabel}>{option.preview?.renderName === undefined ? option.name ?? option.label : option.preview.renderName(option.name ?? option.label)}</span></Primitive.ItemText>}
                    description={option.description}
                    compact
                  >{(content, choice) => <ChoiceCard preview={option.preview}><Primitive.Item
                    {...choice}
                    value={option.value}
                    disabled={option.disabled}
                    aria-keyshortcuts={option.help === undefined || option.disabled === true ? undefined : 'F1'}
                    onFocus={(event) => { lastItem.current = event.currentTarget; setHighlighted(option.value); }}
                    className="choice-option focus-inset flex min-h-8 min-w-0 flex-1 cursor-default items-center gap-2 rounded-sm px-2 py-1 select-none data-[disabled]:text-text-muted data-[highlighted]:bg-bg-muted"
                  >
                    <span className="flex size-4 shrink-0 items-center justify-center text-accent-text">
                      <Primitive.ItemIndicator>
                        <Check aria-hidden />
                      </Primitive.ItemIndicator>
                    </span>
                    {content}
                  </Primitive.Item></ChoiceCard>}</Choice></div>
                ))}
              </Primitive.Viewport>
              {guides.length > 0 && <div className="flex flex-col gap-2 border-t border-border p-2">
                {guides.map(({ option, help: guide }) => <Choice presentation="picker" key={option.value} name={option.name ?? option.label}
                  help={{ ...guide, onOpen: () => { changeOpen(false); guide.onOpen(); } }} compact />)}
              </div>}
              {active?.accessory !== undefined && <div data-choice-accessory className="flex min-w-0 items-center border-t border-border p-2">{active.accessory}</div>}
              </ChoiceInspection>
            </Primitive.Content>
          </Primitive.Portal>
        </Primitive.Root>
        </div>
      )}
    </Field>
  );
}
