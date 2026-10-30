/** Locale and timezone-aware time presentation.
 *
 * A time is shown relative while it is under a day old ("3 min. ago") and as a date and a time of day
 * after that, in the time zone of its reader; the exact moment is what a tooltip holds. Everything is
 * formatted by `Intl`, so each language gets its own words and its own order.
 */
import { useSyncExternalStore } from 'react';
import { formatNumber } from '@/lib/format';

/** A second, a minute, an hour and a day, in milliseconds. */
export const SECOND_MS = 1_000;
export const MINUTE_MS = 60 * SECOND_MS;
export const HOUR_MS = 60 * MINUTE_MS;
export const DAY_MS = 24 * HOUR_MS;

/** Under this age a moment is "now": a clock that differs a little from the server's shows no "in 3 seconds". */
const NOW_WINDOW_MS = 45 * SECOND_MS;
/** Today/yesterday have their own labels; days two through seven use a weekday. */
const COMPACT_WEEK_DAYS = 7;

/** How often a relative time on screen is brought up to date: its smallest step is a minute. */
const TICK_MS = 30 * SECOND_MS;

export interface TimeContext {
  /** A BCP 47 language tag. */
  locale: string;
  /** An IANA time zone; the browser's own when absent. */
  timeZone?: string | undefined;
}

type Moment = Date | string | number;

function toDate(moment: Moment): Date {
  return moment instanceof Date ? moment : new Date(moment);
}

/** Formatters already made: making one costs more than using it, and a conversation of thousands of messages asks for the same few. */
const formatters = new Map<string, Intl.DateTimeFormat>();

function zoned(context: TimeContext, options: Intl.DateTimeFormatOptions): Intl.DateTimeFormat {
  const key = `${context.locale}|${context.timeZone ?? ''}|${JSON.stringify(options)}`;
  const known = formatters.get(key);
  if (known !== undefined) return known;
  let made: Intl.DateTimeFormat;
  try {
    made = new Intl.DateTimeFormat(context.locale, { ...options, timeZone: context.timeZone });
  } catch {
    // A zone name this browser does not know: its own zone is better than no time at all.
    made = new Intl.DateTimeFormat(context.locale, options);
  }
  formatters.set(key, made);
  return made;
}

function dayOf(date: Date, context: TimeContext): string {
  return zoned(context, { year: 'numeric', month: '2-digit', day: '2-digit' }).format(date);
}

/** A local calendar day as an ordinal, independent of daylight-saving day lengths. */
export function calendarDay(moment: Moment, context: TimeContext): number {
  const parts = zoned({ ...context, locale: 'en-US' }, { year: 'numeric', month: 'numeric', day: 'numeric', numberingSystem: 'latn', calendar: 'iso8601' }).formatToParts(toDate(moment));
  const part = (type: Intl.DateTimeFormatPartTypes) => Number(parts.find((value) => value.type === type)?.value);
  return Date.UTC(part('year'), part('month') - 1, part('day')) / DAY_MS;
}

/** A complete short row time; the exact date and zone remain in RelativeTime's tooltip. */
export function formatCompactTime(moment: Moment, context: TimeContext, now: Moment = Date.now()): string {
  const date = toDate(moment), current = toDate(now);
  const days = calendarDay(current, context) - calendarDay(date, context);
  const age = current.getTime() - date.getTime();
  if (days === 0 && Math.abs(age) < NOW_WINDOW_MS) return relativeFormat(context.locale).format(0, 'second');
  if (days === 0 && age > 0 && age < HOUR_MS) return unitFormat(context.locale, 'minute', 'short').format(Math.max(1, Math.floor(age / MINUTE_MS)));
  if (days <= 1) return zoned(context, { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date);
  if (days > 1 && days <= COMPACT_WEEK_DAYS) return zoned(context, { weekday: 'short' }).format(date);
  const year = zoned(context, { year: 'numeric' });
  return zoned(context, { month: 'short', day: 'numeric', year: year.format(date) === year.format(current) ? undefined : 'numeric' }).format(date);
}

/** A date and a time of day, with the year only when it is not the present one. */
export function formatDateTime(moment: Moment, context: TimeContext, now: Moment = Date.now()): string {
  const date = toDate(moment);
  const sameYear =
    zoned(context, { year: 'numeric' }).format(date) === zoned(context, { year: 'numeric' }).format(toDate(now));
  return zoned(context, {
    year: sameYear ? undefined : 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

/** The time of day alone when the moment is today, and the date with it otherwise: "14:05", "14 Oct, 14:05". */
export function formatTimeOfDay(moment: Moment, context: TimeContext, now: Moment = Date.now()): string {
  const date = toDate(moment);
  if (dayOf(date, context) === dayOf(toDate(now), context)) {
    return zoned(context, { hour: '2-digit', minute: '2-digit' }).format(date);
  }
  return formatDateTime(date, context, now);
}

/** The exact moment, to the second and with its zone: what a tooltip shows. */
export function formatExact(moment: Moment, context: TimeContext): string {
  return zoned(context, { dateStyle: 'medium', timeStyle: 'long' }).format(toDate(moment));
}

const relativeFormats = new Map<string, Intl.RelativeTimeFormat>();

function relativeFormat(locale: string): Intl.RelativeTimeFormat {
  let made = relativeFormats.get(locale);
  if (made === undefined) {
    made = new Intl.RelativeTimeFormat(locale, { numeric: 'auto', style: 'short' });
    relativeFormats.set(locale, made);
  }
  return made;
}

/** Relative while under a day away, in either direction; a date and a time after that. */
export function formatRelative(moment: Moment, context: TimeContext, now: Moment = Date.now()): string {
  const date = toDate(moment);
  const delta = date.getTime() - toDate(now).getTime();
  const distance = Math.abs(delta);
  if (distance >= DAY_MS) return formatDateTime(date, context, now);

  const relative = relativeFormat(context.locale);
  if (distance < NOW_WINDOW_MS) return relative.format(0, 'second');
  if (distance < HOUR_MS) return relative.format(Math.trunc(delta / MINUTE_MS) || Math.sign(delta), 'minute');
  return relative.format(Math.trunc(delta / HOUR_MS), 'hour');
}

/** A span of time in its two largest units, compact by default or in full words for a labelled duration. */
export function formatSpan(milliseconds: number, context: TimeContext, style: 'units' | 'short' | 'long' | 'clock' = 'units'): string {
  const total = Math.max(0, Math.round(milliseconds / SECOND_MS));
  if (style === 'clock') {
    const hours = Math.floor(total / 3600);
    const minutes = Math.floor((total % 3600) / 60);
    const seconds = total % 60;
    const first = (value: number) => formatNumber(value, context.locale, { useGrouping: false });
    const next = (value: number) => formatNumber(value, context.locale, { useGrouping: false, minimumIntegerDigits: 2 });
    return hours > 0 ? [first(hours), next(minutes), next(seconds)].join(':') : [first(minutes), next(seconds)].join(':');
  }
  const parts: [number, 'day' | 'hour' | 'minute' | 'second'][] = [
    [Math.floor(total / 86_400), 'day'],
    [Math.floor((total % 86_400) / 3_600), 'hour'],
    [Math.floor((total % 3_600) / 60), 'minute'],
    [total % 60, 'second'],
  ];
  const first = parts.findIndex(([value]) => value > 0);
  const shown = first === -1 ? parts.slice(-1) : parts.slice(first, first + 2).filter(([value]) => value > 0);
  return shown.map(([value, unit]) => unitFormat(context.locale, unit, style === 'long' ? 'long' : style === 'short' ? 'short' : 'narrow').format(value)).join(' ');
}

/** A single-unit estimate stays in seconds through ninety seconds, then minutes through ninety minutes. */
const RANGE_UNIT_LIMIT = 90;

/** A duration estimate uses the shared short unit names, rounding outwards so both bounds remain covered. */
export function formatSpanRange(low: number, high: number, context: TimeContext): string {
  const [unit, divisor] = high <= RANGE_UNIT_LIMIT * SECOND_MS ? (['second', SECOND_MS] as const)
    : high <= RANGE_UNIT_LIMIT * MINUTE_MS ? (['minute', MINUTE_MS] as const) : (['hour', HOUR_MS] as const);
  const from = Math.max(1, Math.floor(low / divisor));
  const to = Math.max(from, Math.ceil(high / divisor));
  return from === to ? formatSpan(from * divisor, context, 'short') : unitFormat(context.locale, unit, 'short').formatRange(from, to);
}

const unitFormats = new Map<string, Intl.NumberFormat>();

function unitFormat(locale: string, unit: 'day' | 'hour' | 'minute' | 'second', display: 'short' | 'long' | 'narrow' = 'narrow'): Intl.NumberFormat {
  const key = `${locale}|${unit}|${display}`;
  let made = unitFormats.get(key);
  if (made === undefined) {
    made = new Intl.NumberFormat(locale, { style: 'unit', unit, unitDisplay: display });
    unitFormats.set(key, made);
  }
  return made;
}

/** The time zone this browser is set to. */
export function browserTimeZone(): string {
  return new Intl.DateTimeFormat().resolvedOptions().timeZone;
}

// ---------------------------------------------------------------- a clock for what is on screen

let tick = Date.now();
let timer: ReturnType<typeof setInterval> | undefined;
const watchers = new Set<() => void>();

function watch(onTick: () => void): () => void {
  watchers.add(onTick);
  if (timer === undefined) {
    tick = Date.now();
    timer = setInterval(() => {
      tick = Date.now();
      for (const watcher of watchers) watcher();
    }, TICK_MS);
  }
  return () => {
    watchers.delete(onTick);
    if (watchers.size === 0) {
      clearInterval(timer);
      timer = undefined;
    }
  };
}

/**
 * The present moment, brought up to date twice a minute for everything that shows a relative time:
 * one timer for the page, however many times are on it.
 */
export function useNow(): number {
  return useSyncExternalStore(watch, () => tick);
}

// ---------------------------------------------------------------- a clock for what is running

const SECOND_TICK_MS = 1_000;
let secondNow = Date.now();
let secondTimer: ReturnType<typeof setInterval> | undefined;
const secondWatchers = new Set<() => void>();

function watchSeconds(onTick: () => void): () => void {
  secondWatchers.add(onTick);
  if (secondTimer === undefined) {
    secondNow = Date.now();
    secondTimer = setInterval(() => {
      secondNow = Date.now();
      for (const watcher of secondWatchers) watcher();
    }, SECOND_TICK_MS);
  }
  return () => {
    secondWatchers.delete(onTick);
    if (secondWatchers.size === 0) {
      clearInterval(secondTimer);
      secondTimer = undefined;
    }
  };
}

const noWatch = () => () => undefined;

/**
 * The present moment, brought up to date every second while `running`: the timer of a step or a turn
 * that is under way. One timer serves every such timer on the page; nothing ticks for what is over.
 */
export function useRunningClock(running: boolean): number {
  return useSyncExternalStore(running ? watchSeconds : noWatch, () => (running ? secondNow : 0));
}
