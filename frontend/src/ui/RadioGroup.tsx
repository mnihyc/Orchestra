/** Shared RadioGroup component (forms); primitives are wrapped here.
 *
 * Form choices show their name and supplied explanation together. The arrow keys move between
 * options, and a checked indicator marks the selected one without using a primary button's fill.
 *
 * A compact group shows an option's explanation on hover and on focus, and keeps it as the option's accessible description.
 * An option that cannot be chosen and says why is blocked and not disabled: it is a step of the arrow keys like the others,
 * so that its reason can be reached without a pointer, and nothing chooses it (a press, a key, or the automatic choosing
 * that follows the arrow keys). A blocked option without a reason, or one in a group that is disabled as a whole, is disabled.
 */
import { Check } from 'lucide-react';
import { RadioGroup as Primitive } from 'radix-ui';
import { type ComponentPropsWithRef, type MouseEvent, useId, type ReactNode } from 'react';

import { Choice, hasChoiceDescription, type ChoiceDetails, type ChoiceName } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export type RadioOption<Value extends string> = ChoiceName & ChoiceDetails & {
  value: Value;
  /** A mark before the name. */
  icon?: ReactNode;
  /** The choice cannot be made: `true`, or its reason (also available as its native tooltip). */
  disabled?: ReactNode;
  /** Any other quiet remark that belongs to this choice alone. */
  note?: ReactNode;
};

export interface RadioGroupProps<Value extends string> {
  /** What is being chosen, for a screen reader. */
  label: string;
  value: Value | null;
  onValueChange: (value: Value) => void;
  options: readonly RadioOption<Value>[];
  variant?: 'cards' | 'list';
  disabled?: boolean;
  className?: string;
  presentation?: 'form' | 'compact';
}

function OptionRadio({ descriptionId, ...props }: ComponentPropsWithRef<typeof Primitive.Item> & { descriptionId: string | undefined }) {
  // The tooltip adds its own id beside the persistent description; the same words need not be read twice.
  return <Primitive.Item {...props} aria-describedby={descriptionId ?? props['aria-describedby']} />;
}

export function RadioGroup<Value extends string>({
  label,
  value,
  onValueChange,
  options,
  variant = 'cards',
  disabled = false,
  className,
  presentation = 'form',
}: RadioGroupProps<Value>) {
  const descriptionBase = useId();
  const compact = presentation === 'compact';
  const described = options.map((option) => {
    const off = option.disabled !== undefined && option.disabled !== null && option.disabled !== false;
    const why = typeof option.disabled === 'boolean' ? undefined : option.disabled;
    const about = hasChoiceDescription(option.description) || hasChoiceDescription(option.note) || hasChoiceDescription(why);
    const description = about ? <span>
      {option.description !== undefined && <span>{option.description}</span>}
      {why !== undefined && <>{' '}<span>{why}</span></>}
      {option.note !== undefined && <>{' '}<span>{option.note}</span></>}
    </span> : undefined;
    const reason = typeof option.disabled === 'string' ? option.disabled : off && typeof option.note === 'string' ? option.note : undefined;
    // Compact: the reason is shown on focus as on hover, so the option must be able to take the focus.
    const blocked = compact && off && !disabled && (hasChoiceDescription(why) || hasChoiceDescription(option.note));
    return { option, off, blocked, description, reason };
  });
  return (
    <div data-choice-selector className="min-w-0 max-w-full">
    <Primitive.Root
      aria-label={label}
      value={value ?? ''}
      onValueChange={(next) => onValueChange(next as Value)}
      disabled={disabled}
      className={cx(variant === 'cards' ? 'grid grid-cols-[repeat(auto-fill,minmax(min(100%,14rem),1fr))] gap-3' : 'flex flex-col gap-2', className)}
    >
      {described.map(({ option, off, blocked, description, reason }, index) => {
        const descriptionId = compact && description !== undefined ? `${descriptionBase}-${index}` : undefined;
        return (
        <Choice
          key={option.value}
          name={option.name ?? option.label}
          description={description}
          accessory={option.accessory}
          help={option.help}
          presentation={presentation}
          nameClassName="font-medium"
          className="max-w-full items-start"
        >
          {(content, control) => <>
            <Tooltip content={compact ? description : undefined}>
              <OptionRadio
                {...control}
                descriptionId={descriptionId}
                value={option.value}
                // Both Radix primitives publish data-state; the control owns its checked styling.
                data-state={(value ?? '') === option.value ? 'checked' : 'unchecked'}
                disabled={off && !blocked}
                {...(blocked ? { 'aria-disabled': true, 'data-blocked': '', onClick: (event: MouseEvent) => event.preventDefault() } : {})}
                aria-description={reason}
                title={reason}
                className={cx(
                  'choice-radio flex min-w-0 max-w-full flex-auto border bg-surface text-left text-text transition-colors disabled:cursor-not-allowed disabled:text-text-disabled aria-disabled:cursor-not-allowed aria-disabled:text-text-disabled',
                  compact ? 'min-h-(--control-md) items-center gap-2 rounded-md px-3' : 'items-start gap-3 rounded-lg p-3',
                )}
              >
                {option.icon !== undefined && (
                  <span className={cx('flex shrink-0 text-text-muted [&_svg]:size-5', !compact && 'mt-0.5')} aria-hidden>
                    {option.icon}
                  </span>
                )}
                {content}
                <span aria-hidden className={cx('flex size-4 shrink-0 items-center justify-center rounded-full border border-control-border bg-surface', !compact && 'mt-0.5')}>
                  <Primitive.Indicator className="flex size-4 items-center justify-center rounded-full bg-accent-solid text-text-on-accent">
                    <Check className="size-3" />
                  </Primitive.Indicator>
                </span>
              </OptionRadio>
            </Tooltip>
            {descriptionId !== undefined && <span id={descriptionId} hidden>{description}</span>}
          </>}
        </Choice>
        );
      })}
    </Primitive.Root>
    </div>
  );
}
