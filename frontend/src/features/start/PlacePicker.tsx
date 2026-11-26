/** The one control for where a thread runs: a field that opens a short list, whatever the number of environments.
 *
 * "New environment…" is the first row and opens its own compact form beside the field; the person's recent environments
 * follow, then the rest, the default first. Each row is one line: the name, then its facts (its machine, Default, how it
 * stands where it is not ready, its free room, the machine's size), shortened to that line and whole on hover, with the
 * owner's chip where it is somebody else's; the current one is marked. Those that need a request for access or cannot be
 * used at all follow in groups of their own, each with its reason. A search narrows the list; the list scrolls inside its
 * own frame, so twelve environments and two thousand take the same room on the screen.
 * The dialog that starts a thread and the dialog that changes where one runs both stand on it.
 */
import { Plus } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import { machineCapacityWords } from '@/features/resources/machines/MachineCapacityLine';
import type { EnvironmentStatus, MachineCapacity, PlaceRoom, UserRef } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { recentChoiceRows, usePickerRecents } from '@/lib/pickerRecents';
import { factLine } from '@/ui/Choice';
import { Combobox, type ComboboxGroup, type ComboboxOption } from '@/ui/Combobox';
import { EnvironmentDot } from '@/ui/EnvironmentDot';
import { PlaceCapacity } from './PlaceCapacity';
import { UserChip } from '@/ui/UserChip';

/** The value of the first row, which stands for an environment that does not exist yet. */
export const NEW_PLACE = '\u0000new-environment';

/** Where an environment is listed: those that can be used now, those that ask for access first, and those that cannot be used. */
export type PlaceGroup = 'ready' | 'ask' | 'unavailable';

export interface PlaceRow {
  capacity?: MachineCapacity | null | undefined;
  id: string;
  name: string;
  /** The host or machine it is on, drawn quietly after the name. */
  on: string | null;
  room?: PlaceRoom | null | undefined;
  running?: number | null | undefined;
  /** Whose it is, where it is someone else's; nothing for the person's own. */
  owner?: UserRef | null;
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
  footer?: ReactNode;
  onOpenChange?: (open: boolean) => void;
  size?: 'sm' | 'md';
  open?: boolean;
}

function normal(text: string | null | undefined, locale: string): string {
  return (text ?? '').normalize('NFKC').toLocaleLowerCase(locale);
}

export function PlacePicker({ label, hideLabel = true, value, rows, onPick, newRow, placeholder, disabled = false, fieldClassName, footer, onOpenChange, size, open }: PlacePickerProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { locale } = format;
  const [query, setQuery] = useState('');
  const [listed, setListed] = useState(false);
  const shown = open ?? listed;
  // The person's recent environments, kept by the server with their other recent choices, are read once the pointer or the
  // keyboard reaches the field. The list never waits for them: it opens in the order they had then; the first answer, where
  // the list opened before it, takes its place at once, and a later one at the next opening, so rows do not move while used.
  const [warm, setWarm] = useState(false);
  const recent = usePickerRecents(warm || shown);
  const recentNow = recent.choices?.environment;
  const [opening, setOpening] = useState<{ shown: boolean; keys: readonly string[]; waiting: boolean }>({ shown: false, keys: [], waiting: false });
  if (opening.shown !== shown) setOpening({ shown, keys: shown ? recentNow ?? [] : [], waiting: shown && recent.loading });
  else if (opening.waiting && !recent.loading) setOpening({ shown, keys: recentNow ?? [], waiting: false });
  const wanted = normal(query.trim(), locale);
  const matching = wanted === '' ? rows : rows.filter((row) => [row.name, row.on, ...(row.keywords ?? [])].some((word) => normal(word, locale).includes(wanted)));
  const separator = t('resourceFacts.separator');
  const optionOf = (row: PlaceRow): ComboboxOption<string> => {
    const unavailable = row.group === 'unavailable';
    const reason = unavailable ? row.reason ?? t('start.where.closed') : row.group === 'ask' ? row.reason ?? t('start.where.askAccess') : null;
    return {
      value: row.id,
      label: row.name,
      text: row.name,
      icon: <EnvironmentDot status={row.status} />,
      // One line of facts, the most telling first; what does not fit is whole on hover. A row that cannot be used says why
      // first, under its name, and keeps the same figures to compare.
      description: factLine([
        ...(unavailable ? [reason] : []),
        row.on,
        row.isDefault === true ? t('start.where.default') : null,
        row.state === null ? null : <span className="text-warning-text">{row.state}</span>,
        unavailable ? null : reason,
        <PlaceCapacity room={row.room} running={row.running} />,
        machineCapacityWords(row.capacity, t, format),
      ], separator),
      ...(row.owner ? { accessory: <UserChip user={row.owner} size={24} /> } : {}),
      disabled: unavailable,
    };
  };
  // A recent environment is one the person can start in now; a search shows every match in its own group.
  const recents = wanted === '' ? recentChoiceRows(matching.filter((row) => row.group === 'ready'), opening.keys, (row) => row.id, () => true) : [];
  const promoted = new Set(recents.map((row) => row.id));
  const rest = matching.filter((row) => !promoted.has(row.id));
  const ready = rest.filter((row) => row.group === 'ready');
  const ask = rest.filter((row) => row.group === 'ask');
  const unavailable = rest.filter((row) => row.group === 'unavailable');
  // The words over a group appear only where the list holds more than one kind.
  const mixed = [recents, ready, ask, unavailable].filter((group) => group.length > 0).length > 1;
  const groups: ComboboxGroup<string>[] = [
    ...(newRow === undefined ? [] : [{
      id: 'new', label: '',
      options: [{ value: NEW_PLACE, label: t('start.where.newRow'), text: t('start.where.newRow'), icon: <Plus aria-hidden />,
        ...(typeof newRow.blocked === 'string' ? { disabled: newRow.blocked } : {}) }],
    }]),
    { id: 'recent', label: t('ui.picker.recent'), options: recents.map(optionOf) },
    { id: 'ready', label: mixed ? t('start.where.canUse') : '', options: ready.map(optionOf) },
    { id: 'ask', label: mixed ? t('start.where.askAccess') : '', options: ask.map(optionOf) },
    { id: 'unavailable', label: mixed ? t('start.where.unavailable') : '', options: unavailable.map(optionOf) },
  ];
  const chosen = value === null || value === NEW_PLACE ? undefined : rows.find((row) => row.id === value);
  return <div className="contents" onPointerEnter={() => setWarm(true)} onFocusCapture={() => setWarm(true)}><Combobox<string>
    label={label}
    hideLabel={hideLabel}
    placeholder={placeholder ?? t('start.where.chooseExisting')}
    disabled={disabled}
    fieldClassName={fieldClassName}
    footer={footer}
    size={size}
    open={open}
    query={query}
    onQueryChange={setQuery}
    displayValue={value === NEW_PLACE ? t('start.where.newName') : chosen?.name}
    {...(value === null ? {} : { initialActiveValue: value })}
    groups={groups}
    empty={t('start.where.noMatches')}
    onOpenChange={(next) => { setListed(next); if (!next) setQuery(''); onOpenChange?.(next); }}
    onSelect={(option) => {
      setQuery('');
      if (option.value === NEW_PLACE) { newRow?.onNew(); return; }
      if (rows.some((row) => row.id === option.value && row.group === 'ready')) recent.record({ kind: 'environment', key: option.value });
      onPick(option.value);
    }}
  /></div>;
}
