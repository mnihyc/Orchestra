/** Shared RadioGroup component (forms); primitives are wrapped here.
 *
 * Form choices show their name and supplied explanation together. The arrow keys move between
 * options, and a checked indicator marks the selected one without using a primary button's fill.
 */
import { Check } from 'lucide-react';
import { RadioGroup as Primitive } from 'radix-ui';
import type { ReactNode } from 'react';

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
    return { option, off, description, reason };
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
      {described.map(({ option, off, description, reason }) => {
        return (
        <Choice
          key={option.value}
          name={option.name ?? option.label}
          description={description}
          help={option.help}
          presentation={presentation}
          nameClassName="font-medium"
          className="max-w-full items-start"
        >{(content, control) => <Tooltip content={presentation === 'compact' ? description : undefined}><Primitive.Item
          {...control}
          value={option.value}
          // Both Radix primitives publish data-state; the control owns its checked styling.
          data-state={(value ?? '') === option.value ? 'checked' : 'unchecked'}
          disabled={off}
          aria-description={reason}
          title={reason}
          className={cx(
            'choice-radio flex min-w-0 max-w-full flex-auto items-start gap-3 rounded-lg border bg-surface p-3 text-left text-text transition-colors disabled:cursor-not-allowed disabled:text-text-disabled',
          )}
        >
          {option.icon !== undefined && (
            <span className="mt-0.5 flex shrink-0 text-text-muted [&_svg]:size-5" aria-hidden>
              {option.icon}
            </span>
          )}
          {content}
          <span
            aria-hidden
            className="mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border border-control-border bg-surface"
          >
            <Primitive.Indicator className="flex size-4 items-center justify-center rounded-full bg-accent-solid text-text-on-accent">
              <Check className="size-3" />
            </Primitive.Indicator>
          </span>
        </Primitive.Item></Tooltip>}</Choice>
        );
      })}
    </Primitive.Root>
    </div>
  );
}
