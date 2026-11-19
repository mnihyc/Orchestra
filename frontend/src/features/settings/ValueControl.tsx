/** The control that edits a value, drawn from the shape of its schema: a switch, a whole number with its unit, one word of several, a text, a list, a map, an object, or JSON where nothing fits better.
 *
 * It knows no key. A value that has the shape of "a list of words out of a closed list" is a row of
 * ticks whether it is the forms of an invitation or the tools an agent is offered; a map of switches is
 * a column of switches under names. What it draws is the value it is given and what it hands back is the
 * whole new value, never a part, so that the row that holds it writes one thing.
 */
import { Plus, Trash2 } from 'lucide-react';
import { type ReactNode, useContext, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { SIZE_UNITS, type Shape, bytesOf, emptyOf, isUnit, sizeParts, words } from '@/features/settings/settingsModel';
import { SettingChoiceContext } from '@/features/settings/choiceCopy';
import { choiceOption, settingChoice } from '@/i18n/choices';
import { Button } from '@/ui/Button';
import { Checkbox, CheckboxList } from '@/ui/Checkbox';
import { Choice } from '@/ui/Choice';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { DateField } from '@/ui/DateField';
import { Select } from '@/ui/Select';
import { ChoiceLocator } from '@/ui/ChoiceLocator';
import { Switch } from '@/ui/Switch';
import { Textarea } from '@/ui/Textarea';
import type { SettingDef as SettingWithText } from '@/generated/api-types';

/** How many characters a text may have before it is edited in a box of several lines. */
const LONG_TEXT_CHARS = 200;
/** The shipped percentage is used when returning from either explicit endpoint to a share. */
const PASTE_SHARE_DEFAULT = 25;

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

export interface ValueControlProps {
  display?: SettingWithText['display'];
  shape: Shape;
  value: unknown;
  onChange: (value: unknown) => void;
  /** A draft changed even when its incomplete number or JSON cannot yet be submitted. */
  onEdited?: (() => void) | undefined;
  /** What the value is, for its field and for a screen reader. */
  label: string;
  /** A containing setting row already shows the group name. */
  hideLabel?: boolean;
  /** A truthful default for an unset scalar, supplied by its containing row. */
  placeholder?: string | undefined;
  disabled?: boolean;
  error?: string | undefined;
  /** Says a word of the registry (a value of a choice) in the reader's language, or as it is where it has no translation. */
  say?: (word: string) => string;
  /** Localized names of a structured setting's declared members, where supplied by its feature. */
  memberLabel?: (name: string) => string;
  /** A structured numeric member's declared display, such as zero meaning Off. */
  memberDisplay?: (name: string) => SettingWithText['display'];
  /** Show a structured text or numeric field's own label when its containing row cannot name that field. */
  showLabel?: boolean;
}

// ---------------------------------------------------------------- whole numbers

function IntControl({ shape, value, onChange, onEdited, label, disabled, error, display, placeholder, showLabel }: ValueControlProps & { shape: Extract<Shape, { type: 'int' }> }) {
  const { t } = useTranslation();
  const bytes = shape.unit === 'bytes';
  const held = typeof value === 'number' ? value : null;
  const parts = bytes && held !== null ? sizeParts(held) : { amount: held, unit: 'B' as const };
  // What is typed is kept here; a value that arrives from outside (a cancel, the answer of the server) draws the control anew.
  const [text, setText] = useState(parts.amount === null ? '' : String(parts.amount));
  const [size, setSize] = useState<(typeof SIZE_UNITS)[number]>(parts.unit);
  const [focused, setFocused] = useState(false);
  const off = display?.zero === 'off' && text.trim() !== '' && Number(text) === 0 && !focused;

  const send = (amountText: string, unit: (typeof SIZE_UNITS)[number]) => {
    const amount = Number(amountText);
    if (amountText.trim() === '' || !Number.isFinite(amount) || (!bytes && !Number.isInteger(amount))) return;
    onChange(bytes ? Math.round(bytesOf(amount, unit)) : amount);
  };
  const unitName = isUnit(shape.unit) && !bytes ? t(`settings.editor.units.${shape.unit}`) : null;
  return (
    <div className="flex flex-wrap items-end gap-2">
      <Input
        label={label}
        hideLabel={showLabel !== true}
        type={off ? 'text' : 'number'}
        required
        inputMode="numeric"
        min={bytes ? 0 : (shape.min ?? undefined)}
        max={bytes ? undefined : (shape.max ?? undefined)}
        value={off ? t('common.off') : text}
        placeholder={placeholder}
        disabled={disabled}
        error={error}
        fieldClassName="w-[10rem]"
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onChange={(event) => {
          setText(event.target.value);
          onEdited?.();
          send(event.target.value, size);
        }}
      />
      {bytes && (
        <Select
          label={t('settings.editor.sizeUnit', { name: label })}
          hideLabel
          value={size}
          disabled={disabled}
          fieldClassName="w-[7rem]"
          onValueChange={(unit) => {
            setSize(unit);
            send(text, unit);
          }}
          options={SIZE_UNITS.map((unit) => ({ value: unit, ...choiceOption(t, `sizeUnit.${unit}`) }))}
        />
      )}
      {unitName !== null && <span aria-hidden={off} className={`pb-2 text-base text-text-muted${off ? ' invisible' : ''}`}>{unitName}</span>}
      {held === 0 && display?.zero === 'never' && <span className="pb-2 text-sm text-text-muted">{t('day100.settings.never')}</span>}
      {focused && held === 0 && display?.zero === 'off' && <span className="pb-2 text-sm text-text-muted">{t('common.off')}</span>}
      {focused && held === 0 && display?.zero === 'no_limit' && <span className="pb-2 text-sm text-text-muted">{t('goals.editor.noLimit')}</span>}
    </div>
  );
}

// ---------------------------------------------------------------- lists

/** A list of words out of a closed list, as ticks; the order of the registry, which the value keeps. */
function ChoiceSet({ values, value, onChange, label, hideLabel, disabled, say }: ValueControlProps & { values: readonly string[] }) {
  const { t } = useTranslation();
  const key = useContext(SettingChoiceContext);
  const chosen = Array.isArray(value) ? (value as unknown[]).filter((entry): entry is string => typeof entry === 'string') : [];
  const word = say ?? words;
  return (
    <CheckboxList label={label} hideLabel={hideLabel === true} value={chosen} onValueChange={onChange} disabled={disabled === true}
      options={values.map((entry) => ({ value: entry, ...(settingChoice(t, key, entry) ?? { name: word(entry) }) }))} />
  );
}

/** The same list where the order of what is ticked matters: the chosen ones in their order, with the rest to add. */
function OrderedChoice({ values, value, onChange, label, disabled, say }: ValueControlProps & { values: readonly string[] }) {
  const { t } = useTranslation();
  const key = useContext(SettingChoiceContext);
  const chosen = Array.isArray(value) ? (value as unknown[]).filter((entry): entry is string => typeof entry === 'string') : [];
  const rest = values.filter((entry) => !chosen.includes(entry));
  const word = say ?? words;
  const move = (at: number, by: -1 | 1) => {
    const next = [...chosen];
    const [taken] = next.splice(at, 1);
    if (taken !== undefined) next.splice(at + by, 0, taken);
    onChange(next);
  };
  return (
    <div className="flex flex-col gap-3">
      {chosen.length === 0 ? (
        <EmptyState compact title={t('settings.editor.orderEmpty')} />
      ) : (
        <ol aria-label={label} className="flex flex-col gap-1">
          {chosen.map((entry, at) => (
            <li key={entry} className="flex items-center gap-2 rounded-md border border-border bg-bg-subtle px-3 py-2">
              <span className="w-5 text-sm text-text-subtle tabular-nums">{at + 1}</span>
              <Choice {...(settingChoice(t, key, entry) ?? { name: word(entry) })} className="min-w-0 flex-1 text-base text-text" />
              <Button size="sm" variant="ghost" disabled={disabled === true || at === 0} aria-label={t('settings.editor.earlier', { name: word(entry) })} onClick={() => move(at, -1)}>
                {t('settings.editor.up')}
              </Button>
              <Button size="sm" variant="ghost" disabled={disabled === true || at === chosen.length - 1} aria-label={t('settings.editor.later', { name: word(entry) })} onClick={() => move(at, 1)}>
                {t('settings.editor.down')}
              </Button>
              <IconButton size="sm" label={t('settings.editor.takeOut', { name: word(entry) })} icon={<Trash2 aria-hidden />} disabled={disabled === true} onClick={() => onChange(chosen.filter((other) => other !== entry))} />
            </li>
          ))}
        </ol>
      )}
      {rest.length > 0 && (
        <div className="flex flex-wrap gap-2">
          {rest.map((entry) => (
            <Choice key={entry} {...(settingChoice(t, key, entry) ?? { name: word(entry) })}>
              {(content, control) => <Button {...control} size="sm" icon={<Plus aria-hidden />} disabled={disabled === true} onClick={() => onChange([...chosen, entry])}>{content}</Button>}
            </Choice>
          ))}
        </div>
      )}
    </div>
  );
}

/** A list of free words, one to a line. */
function Lines({ value, onChange, label, disabled, error }: ValueControlProps) {
  const lines = Array.isArray(value) ? (value as unknown[]).filter((entry): entry is string => typeof entry === 'string') : [];
  const [text, setText] = useState(lines.join('\n'));
  return (
    <Textarea
      label={label}
      hideLabel
      rows={Math.min(8, Math.max(3, lines.length + 1))}
      value={text}
      disabled={disabled}
      error={error}
      className="font-mono"
      spellCheck={false}
      onChange={(event) => {
        setText(event.target.value);
        onChange(event.target.value.split('\n').map((line) => line.trim()).filter((line) => line !== ''));
      }}
    />
  );
}

// ---------------------------------------------------------------- maps and objects

/** A map under names: a row for each name, with the control of its value and a way to take it out. */
function Rows({ shape, value, onChange, onEdited, label, disabled, say }: ValueControlProps & { shape: Extract<Shape, { type: 'map' }> }) {
  const { t } = useTranslation();
  const held = isRecord(value) ? value : {};
  const names = Object.keys(held);
  const [added, setAdded] = useState('');
  const taken = added.trim() === '' || added.trim() in held;
  return (
    <div className="flex flex-col gap-3">
      {names.length === 0 && <EmptyState compact title={t('settings.editor.mapEmpty')} />}
      {names.map((name) => (
        <div key={name} className="flex flex-wrap items-start gap-3">
          <span className={`w-[12rem] shrink-0 pt-2 text-base wrap-anywhere text-text ${shape.byId ? 'font-mono text-sm' : ''}`}>{name}</span>
          <div className="min-w-0 flex-1 basis-[min(100%,14rem)]">
            <ValueControl
              shape={shape.value}
              value={held[name]}
              label={t('settings.editor.entry', { key: label, name })}
              onEdited={onEdited}
              disabled={disabled === true}
              {...(say === undefined ? {} : { say })}
              onChange={(next) => onChange({ ...held, [name]: next })}
            />
          </div>
          <IconButton
            size="sm"
            label={t('settings.editor.takeOut', { name })}
            icon={<Trash2 aria-hidden />}
            disabled={disabled === true}
            onClick={() => onChange(Object.fromEntries(Object.entries(held).filter(([other]) => other !== name)))}
          />
        </div>
      ))}
      <div className="flex flex-wrap items-end gap-2">
        <Input
          label={t('settings.editor.newName', { key: label })}
          hideLabel
          placeholder={shape.byId ? t('settings.editor.newId') : t('settings.editor.newNamePlaceholder')}
          mono={shape.byId}
          value={added}
          disabled={disabled}
          fieldClassName="w-[16rem]"
          onChange={(event) => setAdded(event.target.value)}
        />
        <Button
          size="sm"
          icon={<Plus aria-hidden />}
          disabled={disabled === true || taken}
          onClick={() => {
            onChange({ ...held, [added.trim()]: emptyOf(shape.value) });
            setAdded('');
          }}
        >
          {t('settings.editor.add')}
        </Button>
      </div>
    </div>
  );
}

/** An object with members of its own: a field for each, and the optional ones are put in only when asked for. */
function Members({ shape, value, onChange, onEdited, label, disabled, say, memberLabel, memberDisplay }: ValueControlProps & { shape: Extract<Shape, { type: 'object' }> }) {
  const { t } = useTranslation();
  const held = isRecord(value) ? value : {};
  return (
    <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
      {shape.members.map((member) => {
        const present = member.name in held;
        const name = memberLabel?.(member.name) ?? words(member.name);
        return (
          <div key={member.name} className={`flex min-w-0 flex-col gap-1${memberLabel !== undefined && member.shape.type === 'object' ? ' sm:col-span-2' : ''}`}>
            {memberLabel !== undefined && member.required && member.shape.type === 'object' && <span className="text-sm text-text">{name}</span>}
            {!member.required && (
              <Checkbox
                label={t('settings.editor.memberSet', { name })}
                checked={present}
                disabled={disabled === true}
                onCheckedChange={(on) =>
                  onChange(on ? { ...held, [member.name]: emptyOf(member.shape) } : Object.fromEntries(Object.entries(held).filter(([other]) => other !== member.name)))
                }
              />
            )}
            {(member.required || present) && (
              <ValueControl
                shape={member.shape}
                value={held[member.name]}
                label={memberLabel === undefined ? t('settings.editor.entry', { key: label, name }) : name}
                showLabel={memberLabel !== undefined && member.required}
                display={memberDisplay?.(member.name)}
                {...(memberLabel === undefined ? {} : { memberLabel })}
                {...(memberDisplay === undefined ? {} : { memberDisplay })}
                onEdited={onEdited}
                disabled={disabled === true}
                {...(say === undefined ? {} : { say })}
                onChange={(next) => onChange({ ...held, [member.name]: next })}
              />
            )}
          </div>
        );
      })}
    </div>
  );
}

/** Anything else, as JSON; a value that does not read is said so beside the box, and not sent. */
function Json({ value, onChange, onEdited, label, disabled, error }: ValueControlProps) {
  const { t } = useTranslation();
  const written = JSON.stringify(value ?? null, null, 2);
  const [text, setText] = useState(written);
  const [wrong, setWrong] = useState(false);
  return (
    <Textarea
      label={label}
      hideLabel
      rows={Math.min(14, Math.max(3, written.split('\n').length + 1))}
      value={text}
      disabled={disabled}
      error={wrong ? t('settings.editor.jsonWrong') : error}
      hint={t('settings.editor.jsonHint')}
      className="font-mono"
      spellCheck={false}
      onChange={(event) => {
        setText(event.target.value);
        onEdited?.();
        try {
          onChange(JSON.parse(event.target.value) as unknown);
          event.target.setCustomValidity('');
          setWrong(false);
        } catch {
          event.target.setCustomValidity(t('settings.editor.jsonWrong'));
          setWrong(true);
        }
      }}
    />
  );
}

/** The keys whose list is read in order, and not as a set. */
export const ORDERED_KEYS: ReadonlySet<string> = new Set(['binding.prefer']);

export function ValueControl(props: ValueControlProps & { ordered?: boolean }): ReactNode {
  const { shape, value, onChange, label, disabled, error, say, placeholder } = props;
  const { t } = useTranslation();
  const key = useContext(SettingChoiceContext);
  if (key === 'composer.paste_inline_context_share' && shape.type === 'int') {
    const mode = value === 0 ? 'file' : value === 100 ? 'inline' : 'share';
    return <div className="flex min-w-0 flex-wrap items-end gap-2"><Select label={label} hideLabel value={mode} disabled={disabled}
      onValueChange={(next) => onChange(next === 'file' ? 0 : next === 'inline' ? 100 : PASTE_SHARE_DEFAULT)}
      options={[{value:'file',name:t('uploads.alwaysFile')},{value:'share',name:t('uploads.contextShare')},{value:'inline',name:t('uploads.alwaysInline')}]} />
      {mode === 'share' && <IntControl {...props} shape={shape}/>}</div>;
  }
  switch (shape.type) {
    case 'bool': {
      // The key's title stands beside the control already: the switch keeps it for a screen reader and says its state in words.
      const word = say ?? words;
      const state = value === true ? word('on') : word('off');
      return (
        <Switch
          className="w-fit items-center gap-3"
          label={
            <>
              <span className="sr-only">{label}</span>
              <span aria-hidden>{state}</span>
            </>
          }
          checked={value === true}
          disabled={disabled === true}
          onCheckedChange={onChange}
        />
      );
    }
    case 'int':
      return <IntControl {...props} shape={shape} />;
    case 'choice': {
      const word = say ?? words;
      return (
        <ChoiceLocator
          label={label}
          hideLabel
          value={typeof value === 'string' ? value : ''}
          placeholder={placeholder}
          valueLabel={typeof value === 'string' && !shape.values.includes(value) ? word(value) : undefined}
          disabled={disabled === true}
          error={error}
          onValueChange={onChange}
          options={shape.values.map((entry) => ({ value: entry, ...(settingChoice(t, key, entry) ?? { name: word(entry) }) }))}
        />
      );
    }
    case 'text':
      return (shape.maxLength ?? 0) > LONG_TEXT_CHARS ? (
        <Textarea
          label={label}
          hideLabel={props.showLabel !== true}
          rows={6}
          placeholder={placeholder}
          maxLength={shape.maxLength ?? undefined}
          value={typeof value === 'string' ? value : ''}
          disabled={disabled}
          error={error}
          onChange={(event) => onChange(event.target.value)}
        />
      ) : (
        <Input label={label} hideLabel={props.showLabel !== true} placeholder={placeholder} value={typeof value === 'string' ? value : ''} disabled={disabled} error={error} onChange={(event) => onChange(event.target.value)} autoComplete="off" />
      );
    case 'id':
      return (
        <Input
          label={label}
          hideLabel
          mono
          placeholder={placeholder ?? t('settings.editor.idPlaceholder')}
          value={typeof value === 'string' ? value : ''}
          disabled={disabled}
          error={error}
          onChange={(event) => onChange(event.target.value.trim())}
          autoComplete="off"
        />
      );
    case 'time':
      return <DateField label={label} hideLabel placeholder={placeholder} type="datetime-local" value={typeof value === 'string' ? value.slice(0, 16) : ''} disabled={disabled} error={error} onValueChange={(value) => onChange(value === '' ? '' : new Date(value).toISOString())} />;
    case 'list':
      if (shape.item.type === 'choice' && shape.unique) {
        return props.ordered === true ? <OrderedChoice {...props} values={shape.item.values} /> : <ChoiceSet {...props} values={shape.item.values} />;
      }
      if (shape.item.type === 'text' || shape.item.type === 'id') return <Lines {...props} />;
      return <Json {...props} />;
    case 'map':
      if (shape.value.type === 'bool' || shape.value.type === 'text' || shape.value.type === 'int' || shape.value.type === 'choice') return <Rows {...props} shape={shape} />;
      return <Json {...props} />;
    case 'object':
      return shape.open ? <Json {...props} /> : <Members {...props} shape={shape} />;
    case 'nullable':
      return (
        <div className="flex flex-col gap-3">
          <Checkbox label={t('settings.editor.nullableSet', { name: label })} checked={value !== null && value !== undefined} disabled={disabled === true} onCheckedChange={(on) => onChange(on ? emptyOf(shape.of) : null)} />
          {value !== null && value !== undefined && <ValueControl {...props} shape={shape.of} />}
        </div>
      );
    case 'json':
      return <Json {...props} />;
  }
}
