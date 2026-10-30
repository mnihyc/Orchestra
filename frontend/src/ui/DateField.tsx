/** An explicit year-month-day field with a localized, keyboard-accessible calendar; native browser formats never vary. */
import { CalendarDays, ChevronLeft, ChevronRight } from 'lucide-react';
import { useEffect, useId, useRef, useState, type KeyboardEvent } from 'react';
import { useTranslation } from 'react-i18next';

import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Input, type InputProps } from '@/ui/Input';
import { Popover } from '@/ui/Popover';

/** A six-week grid holds every month without moving its footer or clipping trailing days. */
const WEEKS = 6;
const WEEK_DAYS = 7;
/** Four-digit calendar years have a stable, sortable written form. */
const FIRST_YEAR = 1;
const LAST_YEAR = 9999;
/** Choosing a day for an empty local date/time means its beginning, which remains editable. */
const START_OF_DAY = '00:00';
type DateKind = 'date' | 'datetime-local';

function day(year: number, month: number, number: number): Date {
  const result = new Date(0);
  result.setUTCFullYear(year, month, number);
  result.setUTCHours(0, 0, 0, 0);
  return result;
}

function written(date: Date): string {
  return `${String(date.getUTCFullYear()).padStart(4, '0')}-${String(date.getUTCMonth() + 1).padStart(2, '0')}-${String(date.getUTCDate()).padStart(2, '0')}`;
}

function read(text: string, kind: DateKind): Date | null {
  const found = /^(\d{4})-(\d{2})-(\d{2})(?:[T ]([01]\d|2[0-3]):([0-5]\d))?$/.exec(text);
  if (found === null || (kind === 'date' ? found[4] !== undefined : found[4] === undefined)) return null;
  const year = Number(found[1]); const month = Number(found[2]); const number = Number(found[3]);
  const result = day(year, month - 1, number);
  return year >= FIRST_YEAR && year <= LAST_YEAR && written(result) === text.slice(0, 10) ? result : null;
}

function today(): Date {
  const now = new Date();
  return day(now.getFullYear(), now.getMonth(), now.getDate());
}

export interface DateFieldProps extends Omit<InputProps, 'type' | 'value' | 'defaultValue' | 'onChange' | 'min' | 'max' | 'trailing' | 'ref'> {
  type?: DateKind;
  value: string;
  onValueChange: (value: string) => void;
  min?: string | undefined;
  max?: string | undefined;
}

function Calendar({ value, min, max, onPick }: { value: string; min?: string | undefined; max?: string | undefined; onPick: (value: string) => void }) {
  const { t, i18n } = useTranslation();
  const [focus, setFocus] = useState(() => {
    const selected = read(value.slice(0, 10), 'date') ?? today();
    const lower = read(min?.slice(0, 10) ?? '', 'date'); const upper = read(max?.slice(0, 10) ?? '', 'date');
    if (lower !== null && selected.getTime() < lower.getTime()) return lower;
    return upper !== null && selected.getTime() > upper.getTime() ? upper : selected;
  });
  const grid = useRef<HTMLDivElement>(null);
  const heading = useId();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  // The two shipped languages use Sunday-first (English) and Monday-first (Chinese) calendars.
  const firstWeekday = locale.startsWith('zh') ? 1 : 0;
  const first = day(focus.getUTCFullYear(), focus.getUTCMonth(), 1);
  const start = day(first.getUTCFullYear(), first.getUTCMonth(), 1 - (first.getUTCDay() - firstWeekday + WEEK_DAYS) % WEEK_DAYS);
  const stamp = written(focus);
  const monthName = new Intl.DateTimeFormat(locale, { month: 'long', year: 'numeric', timeZone: 'UTC' }).format(first);
  const weekdayName = new Intl.DateTimeFormat(locale, { weekday: 'short', timeZone: 'UTC' });
  const fullName = new Intl.DateTimeFormat(locale, { dateStyle: 'full', timeZone: 'UTC' });
  const allowed = (date: Date) => date.getUTCFullYear() >= FIRST_YEAR && date.getUTCFullYear() <= LAST_YEAR
    && (min === undefined || min === '' || written(date) >= min.slice(0, 10)) && (max === undefined || max === '' || written(date) <= max.slice(0, 10));
  const month = (by: number) => {
    const target = day(focus.getUTCFullYear(), focus.getUTCMonth() + by, 1);
    return day(target.getUTCFullYear(), target.getUTCMonth(), Math.min(focus.getUTCDate(), day(target.getUTCFullYear(), target.getUTCMonth() + 1, 0).getUTCDate()));
  };
  useEffect(() => { grid.current?.querySelector<HTMLButtonElement>('[tabindex="0"]')?.focus(); }, [stamp]);
  const keys = (event: KeyboardEvent<HTMLButtonElement>) => {
    let target: Date;
    switch (event.key) {
      case 'ArrowLeft': target = day(focus.getUTCFullYear(), focus.getUTCMonth(), focus.getUTCDate() - 1); break;
      case 'ArrowRight': target = day(focus.getUTCFullYear(), focus.getUTCMonth(), focus.getUTCDate() + 1); break;
      case 'ArrowUp': target = day(focus.getUTCFullYear(), focus.getUTCMonth(), focus.getUTCDate() - WEEK_DAYS); break;
      case 'ArrowDown': target = day(focus.getUTCFullYear(), focus.getUTCMonth(), focus.getUTCDate() + WEEK_DAYS); break;
      case 'Home': target = day(focus.getUTCFullYear(), focus.getUTCMonth(), focus.getUTCDate() - (focus.getUTCDay() - firstWeekday + WEEK_DAYS) % WEEK_DAYS); break;
      case 'End': target = day(focus.getUTCFullYear(), focus.getUTCMonth(), focus.getUTCDate() + WEEK_DAYS - 1 - (focus.getUTCDay() - firstWeekday + WEEK_DAYS) % WEEK_DAYS); break;
      case 'PageUp': target = month(-1); break;
      case 'PageDown': target = month(1); break;
      default: return;
    }
    event.preventDefault();
    if (allowed(target)) setFocus(target);
  };
  return <div className="w-64 max-w-full">
    <div className="mb-2 flex items-center justify-between gap-2">
      <IconButton size="sm" label={t('ui.date.previousMonth')} icon={<ChevronLeft aria-hidden />} disabled={!allowed(day(first.getUTCFullYear(), first.getUTCMonth(), 0))} onClick={() => setFocus(month(-1))} />
      <span id={heading} className="text-sm font-medium" aria-live="polite">{monthName}</span>
      <IconButton size="sm" label={t('ui.date.nextMonth')} icon={<ChevronRight aria-hidden />} disabled={!allowed(day(first.getUTCFullYear(), first.getUTCMonth() + 1, 1))} onClick={() => setFocus(month(1))} />
    </div>
    <div role="grid" aria-labelledby={heading} ref={grid}>
      <div role="row" className="grid grid-cols-7">
        {Array.from({ length: WEEK_DAYS }, (_, number) => <span role="columnheader" key={number} className="py-1 text-center text-xs text-text-muted">{weekdayName.format(day(start.getUTCFullYear(), start.getUTCMonth(), start.getUTCDate() + number))}</span>)}
      </div>
      {Array.from({ length: WEEKS }, (_, week) => <div role="row" key={week} className="grid grid-cols-7">
        {Array.from({ length: WEEK_DAYS }, (_, number) => {
          const date = day(start.getUTCFullYear(), start.getUTCMonth(), start.getUTCDate() + week * WEEK_DAYS + number);
          const key = written(date); const selected = key === value.slice(0, 10);
          return <span role="gridcell" aria-selected={selected} key={key} className="flex justify-center">
            <button type="button" tabIndex={key === stamp ? 0 : -1} disabled={!allowed(date)} aria-label={fullName.format(date)}
              className={cx('focus-inset my-0.5 flex size-8 items-center justify-center rounded-md border border-transparent text-sm tabular-nums hover:bg-bg-muted disabled:text-text-disabled',
                selected && 'border-control-selected bg-control-selected font-semibold text-control-on-selected', !selected && date.getUTCMonth() !== first.getUTCMonth() && 'text-text-muted')}
              onKeyDown={keys} onClick={() => onPick(key)}>{date.getUTCDate()}</button>
          </span>;
        })}
      </div>)}
    </div>
  </div>;
}

export function DateField({ type = 'date', value, onValueChange, min, max, label, error, disabled, readOnly, required, onBlur, onInvalid, ...props }: DateFieldProps) {
  const { t } = useTranslation();
  const display = (raw: string) => type === 'date' ? raw : raw.replace('T', ' ');
  const [draft, setDraft] = useState({ source: value, text: display(value) });
  const [touched, setTouched] = useState(false);
  const [open, setOpen] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  if (draft.source !== value) setDraft({ source: value, text: display(value) });
  const text = draft.source === value ? draft.text : display(value);
  const issue = (next: string): string => {
    if (next === '') return required === true ? t('ui.date.required') : '';
    if (read(next, type) === null) return t(type === 'date' ? 'ui.date.invalid' : 'ui.date.invalidTime');
    const canonical = next.replace(' ', 'T');
    if (min !== undefined && min !== '' && canonical < min) return t('ui.date.minimum', { date: min.replace('T', ' ') });
    if (max !== undefined && max !== '' && canonical > max) return t('ui.date.maximum', { date: max.replace('T', ' ') });
    return '';
  };
  const pick = (date: string) => {
    if (disabled === true || readOnly === true) return;
    const next = type === 'date' ? date : `${date}T${read(text, type) === null ? START_OF_DAY : text.slice(11)}`;
    setDraft({ source: value, text: display(next) }); onValueChange(next); setOpen(false); setTouched(false);
    input.current?.setCustomValidity(issue(next)); input.current?.focus();
  };
  return <Input {...props} ref={input} label={label} type="text" data-date-field={type} inputMode="text" autoComplete="off" spellCheck={false}
    placeholder={props.placeholder ?? t(type === 'date' ? 'ui.date.format' : 'ui.date.timeFormat')} value={text} required={required} disabled={disabled} readOnly={readOnly}
    error={error ?? (touched ? issue(text) || undefined : undefined)}
    onChange={(event) => {
      const next = event.target.value;
      setDraft({ source: value, text: next }); event.target.setCustomValidity(issue(next));
      if (next === '' || read(next, type) !== null) onValueChange(next.replace(' ', 'T'));
    }}
    onBlur={(event) => { setTouched(true); event.target.setCustomValidity(issue(text)); onBlur?.(event); }}
    onInvalid={(event) => { setTouched(true); onInvalid?.(event); }}
    trailing={<Popover label={t('ui.date.calendar')} open={open && disabled !== true && readOnly !== true} onOpenChange={setOpen} onOpenAutoFocus={(event) => event.preventDefault()}
      trigger={<IconButton size="sm" label={t('ui.date.choose', { field: typeof label === 'string' ? label : t('ui.date.calendar') })} icon={<CalendarDays aria-hidden />} disabled={disabled === true || readOnly === true} />}>
      <Calendar value={text} min={min} max={max} onPick={pick} />
      {required !== true && <Button size="sm" variant="ghost" onClick={() => { setDraft({ source: value, text: '' }); onValueChange(''); setOpen(false); input.current?.setCustomValidity(''); input.current?.focus(); }}>{t('ui.date.clear')}</Button>}
    </Popover>} />;
}
