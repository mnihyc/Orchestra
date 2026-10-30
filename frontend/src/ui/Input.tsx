/** Shared Input component (forms); primitives are wrapped here.
 *
 * One line of text with its label, hint and error. A password is the same line with a button that
 * shows what was typed, for whoever would rather read it than type it twice.
 */
import { Eye, EyeOff } from 'lucide-react';
import { type InputHTMLAttributes, type ReactNode, type Ref, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';
import { Field, type FieldSlots } from '@/ui/Field';
import { Tooltip } from '@/ui/Tooltip';

export interface InputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, 'size'>, FieldSlots {
  /** Something inside the field at its end: a button that acts on what is typed. */
  trailing?: ReactNode;
  /** Classes for the frame round label, field and messages. */
  fieldClassName?: string;
  /** Sets the text in the fixed-width face: a code, a token, a path. */
  mono?: boolean;
  ref?: Ref<HTMLInputElement>;
}

/** The look of every one-line field, shared with the controls that are drawn like one. */
export const fieldBoxClasses =
  'h-(--control-md) w-full rounded-md border border-control-border bg-surface px-3 text-base text-text transition-colors ' +
  'placeholder:text-text-subtle hover:border-control-border ' +
  'aria-invalid:border-danger ' +
  'disabled:cursor-not-allowed disabled:border-control-border disabled:bg-bg-muted disabled:text-text-disabled disabled:hover:border-control-border ' +
  'read-only:bg-bg-subtle';

export function Input({
  label,
  optional,
  hint,
  error,
  hideLabel,
  aside,
  help,
  trailing,
  fieldClassName,
  mono = false,
  className,
  id,
  ref,
  ...rest
}: InputProps) {
  return (
    <Field
      label={label}
      optional={optional}
      hint={hint}
      error={error}
      hideLabel={hideLabel}
      aside={aside}
      help={help}
      className={fieldClassName}
      id={id}
    >
      {(control) => (
        <div className="relative">
          <input
            ref={ref}
            {...control}
            className={cx(fieldBoxClasses, mono && 'font-mono', trailing !== undefined && 'pr-10', className)}
            {...rest}
          />
          {trailing !== undefined && (
            <div className="absolute inset-y-0 right-1 flex items-center">{trailing}</div>
          )}
        </div>
      )}
    </Field>
  );
}

/** A password, hidden as it is typed, with a button that shows it. */
export function PasswordInput(props: Omit<InputProps, 'type' | 'trailing'>) {
  const { t } = useTranslation();
  const [shown, setShown] = useState(false);
  const field = typeof props.label === 'string' ? props.label.trim() : '';
  const name = field === '' || field === t('auth.fields.password')
    ? t(shown ? 'ui.password.hide' : 'ui.password.show')
    : t(shown ? 'ui.password.hideField' : 'ui.password.showField', { field });
  return (
    <Input
      {...props}
      type={shown ? 'text' : 'password'}
      // A password is never corrected, capitalised or kept by the keyboard.
      autoCapitalize="none"
      autoCorrect="off"
      spellCheck={false}
      trailing={
        <Tooltip content={name}>
          <button
            type="button"
            aria-label={name}
            aria-pressed={shown}
            disabled={props.disabled}
            className="inline-flex size-(--control-sm) items-center justify-center rounded-sm text-text-muted transition-colors hover:bg-bg-muted hover:text-text disabled:text-text-disabled"
            onClick={() => setShown((value) => !value)}
          >
            {shown ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
          </button>
        </Tooltip>
      }
    />
  );
}
