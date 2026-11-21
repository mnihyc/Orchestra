/** The one control for where a thread runs: a field that opens a short list, whatever the number of environments.
 *
 * "New environment…" is the first row and opens its own compact form beside the field; the environments follow, name first
 * with the location and other details on a secondary line, and those that need
 * a request for access or cannot be used at all follow in groups of their own, each with its reason. A search narrows the
 * list; the list scrolls inside its own frame, so twelve environments and two thousand take the same room on the screen.
 * The dialog that starts a thread and the dialog that changes where one runs both stand on it.
 */
import { MachineCapacityLine } from '@/features/resources/machines/MachineCapacityLine';
import { Plus } from 'lucide-react';
import { useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';

import type { EnvironmentStatus, MachineCapacity, PlaceRoom, UserRef } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
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
  const { locale } = useFormatContext();
  const [query, setQuery] = useState('');
  const wanted = normal(query.trim(), locale);
  const shown = wanted === '' ? rows : rows.filter((row) => [row.name, row.on, ...(row.keywords ?? [])].some((word) => normal(word, locale).includes(wanted)));
  const optionOf = (row: PlaceRow): ComboboxOption<string> => {
    const unavailable = row.group === 'unavailable';
    const reason = unavailable ? row.reason ?? t('start.where.closed') : row.group === 'ask' ? row.reason ?? t('start.where.askAccess') : null;
    const accessory = (
      <span className="inline-flex min-w-0 max-w-full flex-wrap items-center gap-x-1 text-sm text-text-muted" data-place-beside>
        {row.on && <span className="min-w-0 truncate" title={row.on}>{row.on}</span>}
        {row.owner && <UserChip user={row.owner} size={24} />}
        {row.isDefault === true && <span>{t('start.where.default')}</span>}
        <PlaceCapacity room={row.room} running={row.running} />
        {row.state && <span className="text-warning-text">{row.state}</span>}
        {reason && <span>{reason}</span>}
      </span>
    );
    return {
      value: row.id,
      label: row.name,
      text: row.name,
      ...(row.capacity ? {description:<MachineCapacityLine facts={row.capacity} />} : {}),
      icon: <EnvironmentDot status={row.status} />,
      accessory,
      disabled: unavailable,
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
    accessoryBelow
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
    onOpenChange={(open) => { if (!open) setQuery(''); onOpenChange?.(open); }}
    onSelect={(option) => {
      setQuery('');
      if (option.value === NEW_PLACE) newRow?.onNew();
      else onPick(option.value);
    }}
  />;
}
