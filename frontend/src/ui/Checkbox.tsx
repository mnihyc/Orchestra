/** Shared Checkbox component (forms); primitives are wrapped here.
 *
 * One thing that is so or is not, with the words that say what. The words are part of the control:
 * pressing them ticks it.
 */
import { Check, Minus } from 'lucide-react';
import { Checkbox as Primitive } from 'radix-ui';
import { type ReactNode, useId } from 'react';

import { Choice, hasChoiceDescription, type ChoiceDetails, type ChoiceHelp, type ChoiceName } from '@/ui/Choice';
import { cx } from '@/ui/cx';
import { Tooltip } from '@/ui/Tooltip';

export interface CheckboxProps {
  label: ReactNode;
  /** One-line explanation beneath the label. */
  hint?: ReactNode;
  /** The explanation shown with a form choice. */
  description?: ReactNode;
  /** A toolbar explicitly opts out of a caption. */
  presentation?: 'compact' | 'picker' | 'form';
  /** A hint (the "?" that opens a card) beside the label, for a choice whose effect is elsewhere. */
  help?: ReactNode | ChoiceHelp;
  /** Current consequence, available only on hover/focus, never as a caption. */
  tooltip?: ReactNode;
  checked: boolean | 'indeterminate';
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  name?: string;
  className?: string;
}

export function Checkbox({ label, checked, onCheckedChange, disabled = false, name, className, tooltip, description, hint, help, presentation = 'form' }: CheckboxProps) {
  const id = useId();
  const pickerDescription = presentation === 'compact' ? undefined : description ?? hint;
  const descriptionId = hasChoiceDescription(pickerDescription) ? `${id}-description` : undefined;
  return (
    <div className={cx('inline-grid min-w-0 w-fit max-w-full self-start justify-items-start gap-1', className)}>
    <div className="flex items-start gap-2">
      <Tooltip content={tooltip ?? (presentation === 'compact' ? description ?? hint : undefined)}><Primitive.Root
        id={id}
        name={name}
        checked={checked}
        data-state={checked === 'indeterminate' ? 'indeterminate' : checked ? 'checked' : 'unchecked'}
        disabled={disabled}
        aria-labelledby={`${id}-name`}
        {...(descriptionId === undefined ? {} : { 'aria-describedby': descriptionId })}
        onCheckedChange={(value) => onCheckedChange(value === true)}
        className={cx(
          'choice-toggle mt-0.5 inline-flex size-4 shrink-0 items-center justify-center rounded-sm border transition-colors',
          'border-control-border bg-surface',
          'data-[state=checked]:border-control-selected data-[state=checked]:bg-control-selected data-[state=checked]:text-control-on-selected',
          'data-[state=indeterminate]:border-control-selected data-[state=indeterminate]:bg-control-selected data-[state=indeterminate]:text-control-on-selected',
          // What cannot be changed is quiet, whichever way it stands.
          'disabled:cursor-not-allowed disabled:border-border disabled:bg-bg-muted disabled:text-text-disabled disabled:hover:border-border',
          'disabled:data-[state=checked]:border-border disabled:data-[state=checked]:bg-bg-muted disabled:data-[state=checked]:text-text-disabled',
          'disabled:data-[state=indeterminate]:border-border disabled:data-[state=indeterminate]:bg-bg-muted disabled:data-[state=indeterminate]:text-text-disabled',
        )}
      >
        <Primitive.Indicator className="inline-flex">
          {checked === 'indeterminate' ? (
            <Minus size={12} strokeWidth={3} aria-hidden />
          ) : (
            <Check size={12} strokeWidth={3} aria-hidden />
          )}
        </Primitive.Indicator>
      </Primitive.Root></Tooltip>
      <Choice id={id} htmlFor={id} name={label} description={pickerDescription} help={help} presentation={presentation}
        className="flex-1" nameClassName={disabled ? 'text-text-disabled' : 'text-text'} />
    </div>
    </div>
  );
}

export type CheckboxOption<Value extends string> = ChoiceName & ChoiceDetails & {
  value: Value;
  disabled?: boolean;
};

export interface CheckboxListProps<Value extends string> {
  label: string;
  /** The surrounding setting row already shows this group name. */
  hideLabel?: boolean;
  value: readonly Value[];
  onValueChange: (value: Value[]) => void;
  options: readonly CheckboxOption<Value>[];
  presentation?: CheckboxProps['presentation'];
  disabled?: boolean;
  className?: string;
}

/** Several independent choices keep compact names, matching an individual in-page checkbox. */
export function CheckboxList<Value extends string>({ label, hideLabel = false, value, onValueChange, options, disabled = false, className, presentation = 'form' }: CheckboxListProps<Value>) {
  return <fieldset className={cx('flex min-w-0 flex-col gap-2', className)}>
    <legend className={hideLabel ? 'sr-only' : 'mb-2 text-sm font-medium text-text'}>{label}</legend>
    {options.map((option) => <Checkbox key={option.value} label={option.name ?? option.label}
      description={option.description} help={option.help} presentation={presentation} checked={value.includes(option.value)} disabled={disabled || option.disabled === true}
      onCheckedChange={(checked) => onValueChange(checked ? [...value, option.value] : value.filter((entry) => entry !== option.value))} />)}
  </fieldset>;
}
