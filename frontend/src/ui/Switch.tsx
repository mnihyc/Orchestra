/** Shared Switch component (forms); primitives are wrapped here.
 *
 * Something that is on or off and takes effect as it is switched, unlike a checkbox, which waits for
 * its form to be sent.
 */
import { Switch as Primitive } from 'radix-ui';
import { type ComponentPropsWithoutRef, type ReactNode, useId } from 'react';

import { cx } from '@/ui/cx';
import { Choice, type ChoiceName, type ChoiceProps } from '@/ui/Choice';
import { AdminActTooltip, type AdminActSpec } from '@/ui/AdminAct';

export type SwitchProps = ChoiceName & Omit<ComponentPropsWithoutRef<typeof Primitive.Root>, 'name' | 'children' | 'asChild' | 'defaultChecked'> & {
  admin?: AdminActSpec;
  description?: ReactNode;
  /** One-line explanation beneath the name in a form or settings row. */
  hint?: ReactNode;
  /** Additional help explicitly supplied by the caller. */
  help?: ChoiceProps['help'];
  /** Additional hover/focus explanation, independent of the visible description. */
  tooltip?: ReactNode;
  /** Compact settings keep the complete explanation on hover and focus. */
  presentation?: 'form' | 'compact';
  title?: string;
  'aria-description'?: string;
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  disabled?: boolean;
  className?: string;
};

export function Switch({ name, label, checked, onCheckedChange, disabled = false, className, admin, tooltip, description, hint, help, presentation = 'form', id: suppliedId, ...controlProps }: SwitchProps) {
  const generated = useId();
  const id = suppliedId ?? generated;
  return (
    <div className={cx('flex min-w-0 max-w-full', className)}>
    <Choice name={name ?? label} description={description ?? hint} help={help} presentation={presentation} htmlFor={id}
      className="w-full items-start justify-between gap-4" nameClassName={cx('text-base',disabled?'text-text-disabled':'text-text')}>
      {(content,control)=><>{content}
      <AdminActTooltip spec={admin} content={tooltip ?? (presentation === 'compact' ? description ?? hint : undefined)}><Primitive.Root
        {...controlProps}
        aria-labelledby={controlProps['aria-label'] === undefined ? control['aria-labelledby'] : undefined}
        aria-describedby={[control['aria-describedby'], controlProps['aria-describedby']].filter(Boolean).join(' ') || undefined}
        id={id}
        checked={checked}
        data-state={checked ? 'checked' : 'unchecked'}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
        className={cx(
          'choice-switch relative inline-flex h-5 w-8 shrink-0 items-center rounded-full border border-transparent transition-colors',
          // What cannot be changed is quiet, whichever way it stands.
          'disabled:cursor-not-allowed',
        )}
      >
        {/* The thumb remains white in both themes. */}
        <Primitive.Thumb data-choice-thumb className="block size-4 translate-x-0.5 rounded-full shadow-sm transition-transform data-[state=checked]:translate-x-3" />
      </Primitive.Root></AdminActTooltip>
      </>}
    </Choice>
    </div>
  );
}
