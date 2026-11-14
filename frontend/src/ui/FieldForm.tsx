/** A form rendered from field declarations: text, secret, a file's text, choice, date and maps.
 *
 * What is asked of a person is declared by whoever asks (a sign-in method, an endpoint's format, a
 * notifier) as a list of fields, each with a name, a label, a kind of value and a sentence of help. This
 * draws them with the controls of the design system and knows none of the things they are for. A
 * secret is a hidden input; a secret text takes a paste or a chosen file and afterwards shows only how
 * much was given ("pasted · 2.1 KB"), never the text; a choice searches its complete declaration; a map is rows of a name and a
 * value, the values hidden for a secret map. A field that declares no help has no line under it.
 */
import { Plus, Trash2, X } from 'lucide-react';
import { type ReactNode, useId, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { Field as FieldDeclaration } from '@/generated/api-types';
import { useAppearance } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Input, PasswordInput } from '@/ui/Input';
import { SavedSecret, SecretField } from '@/ui/SecretField';
import { DateField } from '@/ui/DateField';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Textarea } from '@/ui/Textarea';

/** What a field holds: text, a yes or no, or the rows of a map as a name for each value. */
export type FormValue = string | boolean | Record<string, string>;
export type FormValues = Record<string, FormValue>;

/** Retain the declaration form's explicit Not set value and its empty-string wire mapping. */
const NOTHING = '\u0000none';

export interface FieldFormProps {
  fields: readonly FieldDeclaration[];
  values: FormValues;
  onChange: (name: string, value: FormValue) => void;
  /** What is wrong with a field, by its name. */
  errors?: Readonly<Record<string, string>>;
  disabled?: boolean;
  className?: string;
  /**
   * What stands beside a field's label for the help it declares: a hint, which holds the sentence, in place of
   * the line that would stand under the field. A field that declares no help is given nothing to draw.
   */
  helpFor?: (field: FieldDeclaration) => ReactNode;
  /** The secret fields that already hold a saved value: each is shown as a field of dots that is set, with Replace, and never as an empty box. */
  secretsSet?: readonly string[] | undefined;
  /** The secret fields whose saved value is to be taken away with the next save. */
  secretsRemoved?: readonly string[] | undefined;
  /** Remove or keep a saved secret; where this is not given, a saved secret can only be replaced. */
  onSecretAct?: ((name: string, act: 'remove' | 'keep') => void) | undefined;
}

const asText = (value: FormValue | undefined): string => (typeof value === 'string' ? value : '');
const asMap = (value: FormValue | undefined): Record<string, string> =>
  typeof value === 'object' ? value : {};

/** The value a field starts with: what it declares, else nothing. */
export function initialValue(field: FieldDeclaration): FormValue {
  switch (field.type) {
    case 'boolean':
      return field.default === true;
    case 'json':
      return JSON.stringify(field.default ?? {}, null, 2);
    case 'map':
    case 'secret_map':
      return {};
    default:
      return typeof field.default === 'string' ? field.default : '';
  }
}

/** The values the fields start with. */
export function initialValues(fields: readonly FieldDeclaration[]): FormValues {
  return Object.fromEntries(fields.map((field) => [field.name, initialValue(field)]));
}

/** What the person gave, as the text each field is sent as: a field left empty is left out. */
export function textValues(fields: readonly FieldDeclaration[], values: FormValues): Record<string, string> {
  const sent: Record<string, string> = {};
  for (const field of fields) {
    const value = values[field.name];
    if (value === undefined) continue;
    if (typeof value === 'string') {
      if (value !== '') sent[field.name] = value;
    } else if (typeof value === 'boolean') {
      sent[field.name] = String(value);
    } else if (Object.keys(value).length > 0) {
      sent[field.name] = JSON.stringify(value);
    }
  }
  return sent;
}

/** The names of the required fields that have nothing in them. */
export function missingFields(fields: readonly FieldDeclaration[], values: FormValues): string[] {
  return fields
    .filter((field) => {
      if (!field.required) return false;
      const value = values[field.name];
      if (value === undefined) return true;
      if (typeof value === 'string') return value.trim() === '';
      if (typeof value === 'boolean') return false;
      return Object.keys(value).length === 0;
    })
    .map((field) => field.name);
}

function MapRows({
  field,
  value,
  onChange,
  secret,
  disabled,
  error,
  help,
}: {
  help?: ReactNode;
  field: FieldDeclaration;
  value: Record<string, string>;
  onChange: (value: Record<string, string>) => void;
  secret: boolean;
  disabled: boolean;
  error: string | undefined;
}) {
  const { t } = useTranslation();
  const id = useId();
  // Rows are kept as a list so that a name being typed is never merged with another before it is done.
  const rows = Object.entries(value);
  const rename = (from: string, to: string) => {
    const next: Record<string, string> = {};
    for (const [name, text] of rows) next[name === from ? to : name] = text;
    onChange(next);
  };
  return (
    <fieldset className="flex flex-col gap-2" aria-describedby={error === undefined ? undefined : `${id}-error`}>
      <legend className="text-sm font-medium text-text">
        <span className="flex items-center gap-1">
          <span>
            {field.label + (field.required ? '' : ' ' + t('ui.field.optional'))}
          </span>
          {help}
        </span>
      </legend>
      {help === undefined && field.help !== null && <p className="text-sm text-text-muted">{field.help}</p>}
      {rows.map(([name, text], index) => (
        <div key={index} className="flex flex-wrap items-end gap-2">
          <Input
            label={t('resources.fieldForm.name')}
            hideLabel
            placeholder={t('resources.fieldForm.name')}
            value={name}
            disabled={disabled}
            fieldClassName="min-w-0 flex-1"
            onChange={(event) => rename(name, event.target.value)}
            autoComplete="off"
          />
          {secret ? (
            <PasswordInput
              label={t('resources.fieldForm.value')}
              hideLabel
              placeholder={t('resources.fieldForm.value')}
              value={text}
              disabled={disabled}
              fieldClassName="min-w-0 flex-1"
              onChange={(event) => onChange({ ...value, [name]: event.target.value })}
              autoComplete="off"
            />
          ) : (
            <Input
              label={t('resources.fieldForm.value')}
              hideLabel
              placeholder={t('resources.fieldForm.value')}
              value={text}
              disabled={disabled}
              fieldClassName="min-w-0 flex-1"
              onChange={(event) => onChange({ ...value, [name]: event.target.value })}
              autoComplete="off"
            />
          )}
          <IconButton
            label={t('resources.fieldForm.remove', { name: name === '' ? t('resources.fieldForm.row', { n: index + 1 }) : name })}
            icon={<Trash2 aria-hidden />}
            disabled={disabled}
            onClick={() => onChange(Object.fromEntries(rows.filter(([other]) => other !== name)))}
          />
        </div>
      ))}
      <div>
        <Button
          size="sm"
          icon={<Plus aria-hidden />}
          disabled={disabled || '' in value}
          onClick={() => onChange({ ...value, '': '' })}
        >
          {t('resources.fieldForm.addNamed', { name: field.label })}
        </Button>
      </div>
      {error !== undefined && (
        <p id={`${id}-error`} className="text-sm text-danger-text">
          {error}
        </p>
      )}
    </fieldset>
  );
}

function SecretText({
  field,
  value,
  onChange,
  disabled,
  error,
  help,
}: {
  help?: ReactNode;
  field: FieldDeclaration;
  value: string;
  onChange: (value: string) => void;
  disabled: boolean;
  error: string | undefined;
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const file = useRef<HTMLInputElement>(null);
  const id = useId();
  const size = new TextEncoder().encode(value).length;
  const tooLarge = field.max_bytes !== null && size > field.max_bytes;
  if (value !== '') {
    return (
      <div className="flex flex-col gap-1">
        <span className="flex items-center gap-1 text-sm font-medium text-text">
          <span>
            {field.label + (field.required ? '' : ' ' + t('ui.field.optional'))}
          </span>
          {help}
        </span>
        <div className="flex flex-wrap items-center gap-2 rounded-md border border-border bg-bg-subtle px-3 py-2">
          <span className="flex-1 text-base text-text">{t('resources.fieldForm.pasted', { size: formatBytes(size, locale) })}</span>
          <Button size="sm" variant="ghost" icon={<X aria-hidden />} disabled={disabled} onClick={() => onChange('')}>
            {t('resources.fieldForm.clear')}
          </Button>
        </div>
        {(tooLarge || error !== undefined) && (
          <p className="text-sm text-danger-text">{error ?? t('resources.fieldForm.tooLarge', { size: formatBytes(field.max_bytes ?? 0, locale) })}</p>
        )}
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-2">
      <Textarea
        label={field.label}
        optional={!field.required}
        {...(help === undefined ? { hint: field.help ?? undefined } : { help })}
        error={error}
        rows={3}
        value={value}
        disabled={disabled}
        placeholder={t('resources.fieldForm.pastePlaceholder')}
        onChange={(event) => onChange(event.target.value)}
        className="font-mono"
        spellCheck={false}
        autoComplete="off"
      />
      <div>
        <input
          ref={file}
          id={`${id}-file`}
          type="file"
          className="sr-only"
          tabIndex={-1}
          aria-label={t('resources.fieldForm.chooseFile', { label: field.label })}
          disabled={disabled}
          onChange={(event) => {
            const chosen = event.target.files?.[0];
            if (chosen === undefined) return;
            void chosen.text().then(onChange);
            event.target.value = '';
          }}
        />
        <Button size="sm" disabled={disabled} onClick={() => file.current?.click()}>
          {t('resources.fieldForm.chooseFileButton')}
        </Button>
      </div>
    </div>
  );
}

/** Text and map secrets share the same saved-value acts as an ordinary secret field. */
function DeclaredSecret({ field, saved, removed, empty, disabled, help, error, onReset, onAct, children }: {
  field: FieldDeclaration; saved: boolean; removed: boolean; empty: boolean; disabled: boolean;
  help?: ReactNode; error: string | undefined; onReset: () => void;
  onAct: FieldFormProps['onSecretAct']; children: ReactNode;
}) {
  const { t } = useTranslation();
  const [replacing, setReplacing] = useState(false);
  if (saved && !removed && empty && !replacing) return <SavedSecret label={field.label} optional={!field.required} help={help} error={error} disabled={disabled}
    onReplace={() => setReplacing(true)} onRemove={onAct === undefined ? undefined : () => { setReplacing(false); onAct(field.name, 'remove'); }} />;
  return <div className="flex min-w-0 flex-col gap-1">
    {children}
    {saved && !removed && <div><Button size="sm" variant="ghost" disabled={disabled} onClick={() => { onReset(); setReplacing(false); }}>{t('settingsEditors.secret.keep')}</Button></div>}
    {removed && <div className="flex flex-wrap items-center gap-2 text-sm text-text-muted"><span>{t('settingsEditors.secret.removed')}</span>
      {onAct !== undefined && <Button size="sm" variant="ghost" disabled={disabled} onClick={() => { onReset(); setReplacing(false); onAct(field.name, 'keep'); }}>{t('settingsEditors.secret.keepSaved')}</Button>}</div>}
  </div>;
}

export function FieldForm({ fields, values, onChange, errors = {}, disabled = false, className, helpFor, secretsSet = [], secretsRemoved = [], onSecretAct }: FieldFormProps) {
  const { t } = useTranslation();
  return (
    <div className={cx('flex flex-col gap-4', className)}>
      {fields.map((field) => {
        const value = values[field.name];
        const error = errors[field.name];
        // With a hint to hold it, the help the field declares is not drawn again as a line under the field.
        const beside = helpFor?.(field);
        const aside = beside ?? undefined;
        const help = aside === undefined ? (field.help ?? undefined) : undefined;
        switch (field.type) {
          case 'json': {
            const text = asText(value);
            let invalid: string | undefined;
            if (text.trim() !== '') {
              try { JSON.parse(text); } catch { invalid = t('settings.editor.jsonWrong'); }
            }
            return (
              <Textarea
                key={field.name}
                label={field.label}
                optional={!field.required}
                hint={help}
                help={aside}
                error={error ?? invalid}
                value={text}
                disabled={disabled}
                onChange={(event) => onChange(field.name, event.target.value)}
                rows={6}
                spellCheck={false}
              />
            );
          }
          case 'text':
            return (
              <Input
                key={field.name}
                label={field.label}
                optional={!field.required}
                hint={help}
                help={aside}
                error={error}
                value={asText(value)}
                disabled={disabled}
                onChange={(event) => onChange(field.name, event.target.value)}
                autoComplete="off"
              />
            );
          case 'secret':
            return (
              <SecretField
                key={field.name}
                label={field.label}
                optional={!field.required}
                hint={help}
                help={aside}
                error={error}
                value={asText(value)}
                disabled={disabled}
                onValueChange={(next) => onChange(field.name, next)}
                set={secretsSet.includes(field.name)}
                removed={secretsRemoved.includes(field.name)}
                {...(onSecretAct === undefined ? {} : { onRemove: () => onSecretAct(field.name, 'remove'), onKeep: () => onSecretAct(field.name, 'keep') })}
              />
            );
          case 'secret_text':
            return (
              <DeclaredSecret key={field.name} field={field} saved={secretsSet.includes(field.name)} removed={secretsRemoved.includes(field.name)}
                empty={asText(value) === ''} disabled={disabled} help={aside} error={error} onAct={onSecretAct} onReset={() => onChange(field.name, '')}>
              <SecretText
                help={aside}
                field={field}
                value={asText(value)}
                disabled={disabled}
                error={error}
                onChange={(next) => onChange(field.name, next)}
              />
              </DeclaredSecret>
            );
          case 'choice': {
            const choices = field.choices ?? [];
            return (
              <ChoiceLocator
                key={field.name}
                label={field.label}
                optional={!field.required}
                hint={help}
                help={aside}
                error={error}
                value={asText(value) === '' ? NOTHING : asText(value)}
                valueLabel={field.required && asText(value) === '' ? '' : undefined}
                disabled={disabled}
                onValueChange={(next) => onChange(field.name, next === NOTHING ? '' : next)}
                options={[
                  ...(field.required ? [] : [{ value: NOTHING, label: t('resources.fieldForm.notSet') }]),
                  ...choices.map((choice) => ({ value: choice.value, label: choice.label })),
                ]}
              />
            );
          }
          case 'boolean':
            return (
              <Checkbox
                key={field.name}
                label={field.label}
                hint={help}
                help={aside}
                checked={value === true}
                disabled={disabled}
                onCheckedChange={(next) => onChange(field.name, next)}
              />
            );
          case 'date':
            return (
              <DateField
                key={field.name}
                label={field.label}
                optional={!field.required}
                hint={help}
                help={aside}
                error={error}
                value={asText(value)}
                disabled={disabled}
                onValueChange={(value) => onChange(field.name, value)}
              />
            );
          case 'secret_map':
            return <DeclaredSecret key={field.name} field={field} saved={secretsSet.includes(field.name)} removed={secretsRemoved.includes(field.name)}
              empty={Object.keys(asMap(value)).length === 0} disabled={disabled} help={aside} error={error} onAct={onSecretAct} onReset={() => onChange(field.name, {})}>
              <MapRows field={field} help={aside} value={asMap(value)} secret disabled={disabled} error={error} onChange={(next) => onChange(field.name, next)} />
            </DeclaredSecret>;
          case 'map':
            return (
              <MapRows
                key={field.name}
                help={aside}
                field={field}
                value={asMap(value)}
                secret={false}
                disabled={disabled}
                error={error}
                onChange={(next) => onChange(field.name, next)}
              />
            );
        }
      })}
    </div>
  );
}
