/** The one field for a secret: a value that is set shows as a filled field of dots, with "Set" and a Replace act, never as an empty box that looks as if nothing was saved.
 *
 * The value is never read back, so the dots stand for it and carry nothing of it. Replace opens an empty hidden field for the new value and Keep puts the
 * saved one back; where the field can take the saved value away it offers Remove, and says that it will go when saved. A secret that is not set is an empty hidden
 * field, as a password is.
 */
import { Check } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { Input, type InputProps, PasswordInput } from '@/ui/Input';

/** What the field of a saved secret shows in its place. */
export const SECRET_DOTS = '••••••••••••';

export interface SecretFieldProps extends Omit<InputProps, 'type' | 'trailing' | 'value' | 'onChange' | 'readOnly' | 'defaultValue' | 'ref'> {
  /** What is typed to replace the saved value, or to write the first one. */
  value: string;
  onValueChange: (value: string) => void;
  /** A value is saved already. */
  set?: boolean;
  /** Taking the saved value away has been asked for, and happens with the next save. */
  removed?: boolean;
  /** Offers taking the saved value away. */
  onRemove?: (() => void) | undefined;
  /** Takes back a removal that was asked for. */
  onKeep?: (() => void) | undefined;
  /** Hands the field to a form that moves the focus to its first fault. */
  fieldRef?: ((element: HTMLInputElement | null) => void) | undefined;
}

/** A saved secret: its dots, that it is set, and the acts on it. */
export function SavedSecret({ label, optional, hint, help, error, aside, hideLabel, disabled, fieldClassName, onReplace, onRemove }: Pick<SecretFieldProps, 'label' | 'optional' | 'hint' | 'help' | 'error' | 'aside' | 'hideLabel' | 'disabled' | 'fieldClassName' | 'onRemove'> & { onReplace: () => void }) {
  const { t } = useTranslation();
  return <div data-secret-set className="flex min-w-0 flex-wrap items-end gap-2">
    <Input label={label} optional={optional} hint={hint} help={help} error={error} aside={aside} hideLabel={hideLabel} readOnly mono value={SECRET_DOTS} autoComplete="off"
      fieldClassName={fieldClassName ?? 'min-w-0 flex-1 basis-[12rem]'} />
    <span className="inline-flex h-(--control-md) items-center gap-1 text-sm text-text-muted"><Check size={14} aria-hidden />{t('settingsEditors.secret.set')}</span>
    <Button size="sm" disabled={disabled === true} onClick={onReplace}>{t('settingsEditors.secret.replace')}</Button>
    {onRemove !== undefined && <Button size="sm" variant="ghost" disabled={disabled === true} onClick={onRemove}>{t('settingsEditors.secret.remove')}</Button>}
  </div>;
}

export function SecretField({ value, onValueChange, set = false, removed = false, onRemove, onKeep, fieldRef, disabled, label, ...rest }: SecretFieldProps) {
  const { t } = useTranslation();
  const [replacing, setReplacing] = useState(false);
  const field = useRef<HTMLInputElement>(null);
  useEffect(() => { if (replacing) field.current?.focus(); }, [replacing]);
  const saved = set && !removed;
  if (saved && !replacing && value === '') {
    return <SavedSecret label={label} disabled={disabled} onReplace={() => setReplacing(true)} {...(onRemove === undefined ? {} : { onRemove })}
      {...(rest.optional === undefined ? {} : { optional: rest.optional })} hint={rest.hint} help={rest.help} error={rest.error} aside={rest.aside}
      {...(rest.hideLabel === undefined ? {} : { hideLabel: rest.hideLabel })} {...(rest.fieldClassName === undefined ? {} : { fieldClassName: rest.fieldClassName })} />;
  }
  return <div className="flex min-w-0 flex-col gap-1">
    <PasswordInput {...rest} ref={(element) => { field.current = element; fieldRef?.(element); }} label={label} disabled={disabled} value={value} autoComplete="off" onChange={(event) => onValueChange(event.target.value)} />
    {saved && <div><Button size="sm" variant="ghost" disabled={disabled === true} onClick={() => { onValueChange(''); setReplacing(false); }}>{t('settingsEditors.secret.keep')}</Button></div>}
    {removed && <div className="flex flex-wrap items-center gap-2 text-sm text-text-muted"><span>{t('settingsEditors.secret.removed')}</span>
      {onKeep !== undefined && <Button size="sm" variant="ghost" disabled={disabled === true} onClick={onKeep}>{t('settingsEditors.secret.keepSaved')}</Button>}</div>}
  </div>;
}
