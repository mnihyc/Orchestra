/** The one control for where a thread runs: a field that opens a short list, whatever the number of environments.
 *
 * "New environment…" is the first row and opens its own compact form beside the field; the environments follow, one line
 * each (name, then the host or machine in a quiet word, then how it stands only when it is not ready), and those that need
 * a request for access or cannot be used at all follow in groups of their own, each with its reason. A search narrows the
 * list; the list scrolls inside its own frame, so twelve environments and two thousand take the same room on the screen.
 * The dialog that starts a thread and the dialog that changes where one runs both stand on it.
 */
import { Plus } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { EnvironmentStatus } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { Combobox, type ComboboxGroup, type ComboboxOption } from '@/ui/Combobox';
import { EnvironmentDot } from '@/ui/EnvironmentDot';

/** The value of the first row, which stands for an environment that does not exist yet. */
export const NEW_PLACE = '\u0000new-environment';

/** Where an environment is listed: those that can be used now, those that ask for access first, and those that cannot be used. */
export type PlaceGroup = 'ready' | 'ask' | 'unavailable';

export interface PlaceRow {
  id: string;
  name: string;
  /** The host or machine it is on, drawn quietly after the name. */
  on: string | null;
  /** Whose it is, where it is someone else's; nothing for the person's own. */
  owner?: string | null;
  /** How it stands, only where it is not ready. */
  state: string | null;
  group: PlaceGroup;
  /** Why it asks for access or cannot be used, in words. */
  reason: string | null;
  status: EnvironmentStatus;
  /** The default of the place the thread starts in. */
  isDefault?: boolean;
  /** Other words the search matches. */
  keywords?: readonly (string | null | undefined)[];
}

export interface PlacePickerProps {
  /** What the field is called; kept for a screen reader alone where a heading above it says it to everyone else. */
  label: string;
  hideLabel?: boolean;
  /** The environment that is chosen, `NEW_PLACE` for a new one, or `null` while nothing is. */
  value: string | null;
  rows: readonly PlaceRow[];
  onPick: (id: string) => void;
  /** The first row. Left out where no environment can be made here; a string is why it cannot be chosen. */
  newRow?: { onNew: () => void; blocked?: string | null };
  /** What the field says while nothing is chosen. */
  placeholder?: string;
  disabled?: boolean;
  fieldClassName?: string;
}

function normal(text: string | null | undefined, locale: string): string {
  return (text ?? '').normalize('NFKC').toLocaleLowerCase(locale);
}

export function PlacePicker({ label, hideLabel = true, value, rows, onPick, newRow, placeholder, disabled = false, fieldClassName }: PlacePickerProps) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [query, setQuery] = useState('');
  const wanted = normal(query.trim(), locale);
  const shown = wanted === '' ? rows : rows.filter((row) => [row.name, row.on, ...(row.keywords ?? [])].some((word) => normal(word, locale).includes(wanted)));
  const optionOf = (row: PlaceRow): ComboboxOption<string> => {
    const beside = [row.isDefault === true ? t('start.where.default') : null, row.on, row.owner ?? null].filter((word): word is string => word !== null && word !== '').join(t('startFlow.separator'));
    const accessory = beside === '' && row.state === null ? undefined : (
      <span className="min-w-0 truncate text-xs text-text-muted" title={[beside, row.state].filter(Boolean).join(t('startFlow.separator'))} data-place-beside>
        {beside}
        {row.state !== null && <span className="text-warning-text">{beside === '' ? '' : t('startFlow.separator')}{row.state}</span>}
      </span>
    );
    return {
      value: row.id,
      label: row.name,
      text: row.name,
      icon: <EnvironmentDot status={row.status} />,
      ...(accessory === undefined ? {} : { accessory }),
      ...(row.group === 'unavailable' ? { disabled: row.reason ?? t('start.where.closed') } : row.group === 'ask' ? { description: row.reason ?? t('start.where.askAccess') } : {}),
    };
  };
  const ready = shown.filter((row) => row.group === 'ready');
  const ask = shown.filter((row) => row.group === 'ask');
  const unavailable = shown.filter((row) => row.group === 'unavailable');
  // The words over a group appear only where the list holds more than one kind.
  const mixed = [ready, ask, unavailable].filter((group) => group.length > 0).length > 1;
  const groups: ComboboxGroup<string>[] = [
    ...(newRow === undefined ? [] : [{
      id: 'new', label: '',
      options: [{ value: NEW_PLACE, label: t('start.where.newRow'), text: t('start.where.newRow'), icon: <Plus aria-hidden />,
        ...(typeof newRow.blocked === 'string' ? { disabled: newRow.blocked } : {}) }],
    }]),
    { id: 'ready', label: mixed ? t('start.where.canUse') : '', options: ready.map(optionOf) },
    { id: 'ask', label: mixed ? t('start.where.askAccess') : '', options: ask.map(optionOf) },
    { id: 'unavailable', label: mixed ? t('start.where.unavailable') : '', options: unavailable.map(optionOf) },
  ];
  const chosen = value === null || value === NEW_PLACE ? undefined : rows.find((row) => row.id === value);
  return <Combobox<string>
    label={label}
    hideLabel={hideLabel}
    placeholder={placeholder ?? t('start.where.chooseExisting')}
    disabled={disabled}
    fieldClassName={fieldClassName}
    query={query}
    onQueryChange={setQuery}
    displayValue={value === NEW_PLACE ? t('start.where.newName') : chosen?.name}
    {...(value === null ? {} : { initialActiveValue: value })}
    groups={groups}
    empty={t('start.where.noMatches')}
    onOpenChange={(open) => { if (!open) setQuery(''); }}
    onSelect={(option) => {
      setQuery('');
      if (option.value === NEW_PLACE) newRow?.onNew();
      else onPick(option.value);
    }}
  />;
}
