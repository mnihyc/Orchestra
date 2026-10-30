/** Shared Textarea component (forms); primitives are wrapped here.
 *
 * Several lines of text with their label, hint and error: a description, a reason, a note. It is the
 * same field as `Input`, drawn taller, and it grows with what is typed up to a few lines more.
 */
import type { Ref, TextareaHTMLAttributes } from 'react';

import { cx } from '@/ui/cx';
import { Field, type FieldSlots } from '@/ui/Field';
import { fieldBoxClasses } from '@/ui/Input';

export interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement>, FieldSlots {
  /** Classes for the frame round label, field and messages. */
  fieldClassName?: string;
  ref?: Ref<HTMLTextAreaElement>;
}

export function Textarea({
  label,
  optional,
  hint,
  error,
  hideLabel,
  aside,
  help,
  fieldClassName,
  className,
  id,
  rows = 3,
  ref,
  ...rest
}: TextareaProps) {
  // A caller's resize utility replaces the default, without relying on generated CSS order.
  const resizeGiven = className?.split(/\s+/).some((name) => name === 'resize' || name === 'resize-none' || name === 'resize-x' || name === 'resize-y') ?? false;
  return (
    <Field label={label} optional={optional} hint={hint} error={error} hideLabel={hideLabel} aside={aside} help={help} className={fieldClassName} id={id}>
      {(control) => (
        <textarea
          ref={ref}
          {...control}
          rows={rows}
          // A field is one control: its height is its text's, and its reader may make it taller.
          className={cx(fieldBoxClasses, 'h-auto min-h-(--control-md) py-2', !resizeGiven && 'resize-y', className)}
          {...rest}
        />
      )}
    </Field>
  );
}
