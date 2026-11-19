/** The frame a form control stands in: its label, a hint under it, and what is wrong with it.
 *
 * Every control of a form is named by a visible label, explained where it needs it, and says what is
 * wrong beside itself and not at the top of the form. The frame ties the three to the control, so
 * that a screen reader says them with it.
 */
import { CircleAlert } from 'lucide-react';
import { type ReactNode, useId } from 'react';
import { useTranslation } from 'react-i18next';

import { cx } from '@/ui/cx';

export interface FieldSlots {
  label: ReactNode;
  /** Marks a field that may be left empty. */
  optional?: boolean | undefined;
  /** One line under the control that helps before anything is wrong. */
  hint?: ReactNode;
  /** What is wrong. While it is set the control is marked invalid and the hint gives way to it. */
  error?: ReactNode;
  /** Keeps the label for screen readers only, where a heading right above says the same. */
  hideLabel?: boolean | undefined;
  /** Something at the far end of the label's line: a link that helps with this field. */
  aside?: ReactNode;
  /** A hint (the "?" that opens a card) beside the label's words, for a field that is not plain. It stands outside the label, so it is not part of the field's name. */
  help?: ReactNode;
}

export interface FieldProps extends FieldSlots {
  className?: string | undefined;
  /** The id the control takes, when the caller needs to choose it. */
  id?: string | undefined;
  children: (control: { id: string; 'aria-labelledby': string; 'aria-describedby': string | undefined; 'aria-invalid': true | undefined }) => ReactNode;
}

export function Field({ label, optional, hint, error, hideLabel, aside, help, className, id, children }: FieldProps) {
  const { t } = useTranslation();
  const generated = useId();
  const controlId = id ?? generated;
  const labelId = `${controlId}-label`;
  const hintId = `${controlId}-hint`;
  const errorId = `${controlId}-error`;
  const hasError = error !== undefined && error !== null && error !== false && error !== '';
  const hasHint = !hasError && hint !== undefined && hint !== null && hint !== false && hint !== '';

  return (
    <div className={cx('flex flex-col gap-1', className)}>
      <div className={cx('flex items-baseline justify-between gap-2', hideLabel === true && 'sr-only')}>
        <span className="flex min-w-0 items-center gap-1">
          <label id={labelId} htmlFor={controlId} className="text-sm font-medium text-text">
            {typeof label === 'string' ? label + (optional === true ? ' ' + t('ui.field.optional') : '') : <>{label}{optional === true && <> {t('ui.field.optional')}</>}</>}
          </label>
          {help !== undefined && help !== null && help !== false && <span className="flex shrink-0 items-center">{help}</span>}
        </span>
        {aside !== undefined && <span className="text-sm">{aside}</span>}
      </div>
      {children({
        id: controlId,
        'aria-labelledby': labelId,
        'aria-describedby': hasError ? errorId : hasHint ? hintId : undefined,
        'aria-invalid': hasError ? true : undefined,
      })}
      {hasHint && (
        <p id={hintId} className="text-sm text-text-muted">
          {hint}
        </p>
      )}
      {/* The place is always there, so that what appears in it is announced. */}
      <p id={errorId} aria-live="polite" className={cx('text-sm text-danger-text', !hasError && 'hidden')}>
        {hasError && (
          <span className="flex items-start gap-1">
            <CircleAlert className="mt-0.5" aria-hidden />
            <span>{error}</span>
          </span>
        )}
      </p>
    </div>
  );
}
