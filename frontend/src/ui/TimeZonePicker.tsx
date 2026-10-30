/** A searchable time-zone choice, with this device first and current offsets beside local times. */
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { TimezoneMode } from '@/generated/api-types';
import { choiceCopy } from '@/i18n/choices';
import { browserTimeZone, useNow } from '@/lib/time';
import { Combobox, type ComboboxGroup, type ComboboxOption } from '@/ui/Combobox';
import type { FieldSlots } from '@/ui/Field';

const DEVICE = 'device';
const UTC = 'UTC';
const REGIONS = ['Africa', 'America', 'Antarctica', 'Arctic', 'Asia', 'Atlantic', 'Australia', 'Europe', 'Indian', 'Pacific', 'Other'] as const;
type Region = typeof REGIONS[number];

interface Zone {
  id: string;
  name: string;
  region: Region;
  offset: string;
  minutes: number;
  time: string;
  abbreviation: string;
  search: string;
}

interface ZoneFormats {
  local: Intl.DateTimeFormat;
  offset: Intl.DateTimeFormat;
  english: Intl.DateTimeFormat;
  regional: Intl.DateTimeFormat;
}

// The browser's zone catalogue is small and finite. Reuse its formatters as the shared clock ticks.
const formats = new Map<string, ZoneFormats>();

function normalize(value: string): string {
  return value.toLocaleLowerCase().replaceAll('_', ' ').replaceAll('−', '-').trim();
}

/** A signed offset is compared as minutes, so +8 cannot accidentally match +08:45 or -08:00. */
function offsetMinutes(value: string): number | null {
  const found = /^(?:(?:utc|gmt)\s*)?([+-])\s*(\d{1,2})(?::?(\d{2}))?$/iu.exec(value);
  if (found === null) return /^(?:utc|gmt)$/iu.test(value) ? 0 : null;
  const hours = Number(found[2]), minutes = Number(found[3] ?? 0);
  if (hours > 23 || minutes > 59) return null;
  return (found[1] === '-' ? -1 : 1) * (hours * 60 + minutes);
}

function zoneAt(id: string, locale: string, now: number): Zone | null {
  try {
    const key = `${locale}|${id}`;
    let known = formats.get(key);
    if (known === undefined) {
      known = {
        local: new Intl.DateTimeFormat(locale, { timeZone: id, hour: '2-digit', minute: '2-digit', hourCycle: 'h23', timeZoneName: 'short' }),
        offset: new Intl.DateTimeFormat('en', { timeZone: id, timeZoneName: 'longOffset' }),
        english: new Intl.DateTimeFormat('en', { timeZone: id, timeZoneName: 'long' }),
        // Asian English keeps familiar short forms such as SGT/HKT where generic English uses GMT+8.
        regional: new Intl.DateTimeFormat('en-SG', { timeZone: id, timeZoneName: 'short' }),
      };
      formats.set(key, known);
    }
    const parts = known.local.formatToParts(now);
    const abbreviation = parts.find((part) => part.type === 'timeZoneName')?.value ?? '';
    const rawOffset = known.offset.formatToParts(now).find((part) => part.type === 'timeZoneName')?.value ?? UTC;
    const minutes = offsetMinutes(rawOffset) ?? 0;
    const offset = `UTC${minutes < 0 ? '-' : '+'}${String(Math.floor(Math.abs(minutes) / 60)).padStart(2, '0')}:${String(Math.abs(minutes) % 60).padStart(2, '0')}`;
    const english = known.english.formatToParts(now).find((part) => part.type === 'timeZoneName')?.value ?? '';
    const regional = known.regional.formatToParts(now).find((part) => part.type === 'timeZoneName')?.value ?? '';
    // Intl's short names vary by language. English initials also find EDT, CEST, IST, etc. in either catalogue.
    const initials = english.split(/\s+/u).map((word) => word[0] ?? '').join('');
    const name = id.replaceAll('_', ' ');
    return {
      id, name,
      region: REGIONS.find((region) => id.startsWith(`${region}/`)) ?? 'Other',
      offset, minutes, abbreviation,
      time: parts.filter((part) => part.type !== 'timeZoneName').map((part) => part.value).join('').trim(),
      search: normalize(`${id} ${name} ${offset} ${abbreviation} ${regional} ${english} ${initials}`),
    };
  } catch {
    // A saved alias newer than this browser's catalogue stays named in the field, without breaking the list.
    return null;
  }
}

export interface TimeZoneValue {
  timeZone: string;
  mode: TimezoneMode;
}

export interface TimeZonePickerProps extends FieldSlots {
  value: string;
  mode: TimezoneMode;
  onValueChange: (value: TimeZoneValue) => void;
  disabled?: boolean;
  id?: string;
}

export function TimeZonePicker({ value, mode, onValueChange, disabled = false, hint, ...field }: TimeZonePickerProps) {
  const { t, i18n } = useTranslation();
  const [query, setQuery] = useState('');
  const now = useNow();
  const deviceZone = browserTimeZone();
  const inUse = mode === 'device' ? deviceZone : value;
  const locale = i18n.resolvedLanguage ?? 'en';
  const zones = useMemo(() => {
    const known = typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : [];
    return [...new Set([value, deviceZone, UTC, ...known])]
      .sort((one, other) => one.localeCompare(other))
      .flatMap((zone) => {
        const row = zoneAt(zone, locale, now);
        return row === null ? [] : [row];
      });
  }, [value, deviceZone, locale, now]);
  const byId = new Map(zones.map((zone) => [zone.id, zone]));
  const device = byId.get(deviceZone);
  const deviceCopy = choiceCopy(t, 'timeZone.device', { zone: deviceZone, offset: device?.offset ?? '', time: device?.time ?? '' });
  const needle = normalize(query);
  const offset = offsetMinutes(needle);
  const matches = (zone: Zone) => {
    if (offset !== null) return zone.minutes === offset;
    const text = `${zone.search} ${normalize(t(`ui.timeZonePicker.regions.${zone.region}`))}`;
    return needle.split(/\s+/u).every((word) => text.includes(word));
  };
  const option = (zone: Zone): ComboboxOption<string> => {
    const copy = choiceCopy(t, 'timeZone.fixed', { zone: zone.name, offset: zone.offset, time: zone.time, abbreviation: zone.abbreviation });
    return { value: zone.id, label: copy.name, text: copy.name, description: copy.description };
  };
  const pinned = [...new Set([inUse, UTC])];
  const groups: ComboboxGroup<string>[] = [{
    id: 'quick', label: t('ui.timeZonePicker.quick'),
    options: [
      ...(device !== undefined && (matches(device) || offset === null && normalize(deviceCopy.name).includes(needle))
        ? [{ value: DEVICE, label: deviceCopy.name, text: deviceCopy.name, description: deviceCopy.description }] : []),
      ...pinned.flatMap((id) => {
        const zone = byId.get(id);
        return zone !== undefined && matches(zone) ? [option(zone)] : [];
      }),
    ],
  }, ...REGIONS.map((region) => ({
    id: region, label: t(`ui.timeZonePicker.regions.${region}`),
    options: zones.filter((zone) => zone.region === region && !pinned.includes(zone.id) && matches(zone)).map(option),
  }))];
  const displayValue = mode === 'device'
    ? t('ui.timeZonePicker.deviceValue', { name: deviceCopy.name, zone: deviceZone, offset: device?.offset ?? '' })
    : value.replaceAll('_', ' ');
  const choose = (chosen: ComboboxOption<string>) => {
    setQuery('');
    onValueChange(chosen.value === DEVICE ? { timeZone: deviceZone, mode: 'device' } : { timeZone: chosen.value, mode: 'fixed' });
  };

  return <Combobox {...field} hint={hint} placeholder={t('ui.timeZonePicker.search')} query={query} onQueryChange={setQuery}
    displayValue={displayValue} groups={groups} disabled={disabled} empty={t('ui.timeZonePicker.empty')}
    onSelect={choose} />;
}
