/** An explicit authored time previews the native once/daily/weekly calendar, never a guessed minute. */
import type { InputTiming } from '@/generated/api-types';
import { wallInstants } from '@/features/settings/personal/away/time';

const MINUTE_MS = 60_000;
const DAY_MS = 24 * 60 * 60_000;
/** The native scheduler considers today through the next full weekly cycle. */
const WEEK_DAYS = 7;
export interface TimingFields { mode: 'once' | 'repeat'; frequency: 'daily' | 'weekly'; date: string; time: string; zone: string; weekdays: number[]; fold: '' | 'first' | 'second' }
export interface TimingPreview { timing: InputTiming | null; at: string | null; skipped: string | null; error: 'missing' | 'invalid' | 'future' | 'ambiguous' | 'weekdays' | null; ambiguous: boolean }
const empty = (error: TimingPreview['error'], ambiguous = false): TimingPreview => ({ timing: null, at: null, skipped: null, error, ambiguous });
function civilNow(zone: string, now: number): string {
  const parts = new Intl.DateTimeFormat('en-GB', { timeZone: zone, year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit', hourCycle: 'h23', calendar: 'iso8601', numberingSystem: 'latn' }).formatToParts(now);
  const part = (name: string) => parts.find((row) => row.type === name)?.value ?? '';
  return `${part('year')}-${part('month')}-${part('day')}T${part('hour')}:${part('minute')}`;
}
export function timingPreview(fields: TimingFields, now: number): TimingPreview {
  if (fields.time === '' || fields.zone === '' || fields.mode === 'once' && fields.date === '') return empty('missing');
  if (fields.mode === 'once') {
    const found = wallInstants(fields.date, fields.time, fields.zone);
    if (found === null || found.length === 0) return empty('invalid');
    if (found.length > 1 && fields.fold === '') return empty('ambiguous', true);
    const at = fields.fold === 'second' ? found.at(-1) : found[0];
    if (at === undefined || at <= now) return empty('future', found.length > 1);
    const instant = new Date(at).toISOString();
    return { timing: { kind: 'once', at: instant, zone: fields.zone }, at: instant, skipped: null, error: null, ambiguous: found.length > 1 };
  }
  if (fields.frequency === 'weekly' && fields.weekdays.length === 0) return empty('weekdays');
  try {
    const current = civilNow(fields.zone, now), today = current.slice(0, 10);
    const start = Date.parse(`${today}T00:00:00.000Z`);
    const timing: InputTiming = { kind: 'repeat', frequency: fields.frequency, time: fields.time, zone: fields.zone, weekdays: fields.frequency === 'daily' ? [] : [...fields.weekdays].sort((a, b) => a - b) };
    for (let offset = 0; offset <= WEEK_DAYS; offset += 1) {
      const date = new Date(start + offset * DAY_MS), day = date.getUTCDay() || WEEK_DAYS;
      if (fields.frequency === 'weekly' && !fields.weekdays.includes(day)) continue;
      const label = date.toISOString().slice(0, 10), found = wallInstants(label, fields.time, fields.zone);
      if (found === null) return empty('invalid');
      if (found.length === 0) {
        if (`${label}T${fields.time}` > current) return { timing, at: null, skipped: `${label} ${fields.time}`, error: null, ambiguous: false };
        continue;
      }
      // A repeating fold chooses the earlier occurrence once, even after that first instant has passed.
      const first = found[0];
      if (first !== undefined && first > now) return { timing, at: new Date(first).toISOString(), skipped: null, error: null, ambiguous: false };
    }
    return empty('invalid');
  } catch { return empty('invalid'); }
}
export function fieldsForTiming(timing?: InputTiming): TimingFields {
  const base: TimingFields = { mode: 'once', frequency: 'daily', date: '', time: '', zone: timing?.zone ?? '', weekdays: [], fold: '' };
  if (timing?.kind === 'repeat') return { ...base, mode: 'repeat', frequency: timing.frequency, time: timing.time, weekdays: [...timing.weekdays] };
  if (timing?.kind === 'once') {
    const current = civilNow(timing.zone, Date.parse(timing.at));
    const found = wallInstants(current.slice(0, 10), current.slice(11, 16), timing.zone);
    return { ...base, date: current.slice(0, 10), time: current.slice(11, 16), fold: found !== null && found.length > 1 ? found[0] === Math.floor(Date.parse(timing.at) / MINUTE_MS) * MINUTE_MS ? 'first' : 'second' : '' };
  }
  return base;
}
