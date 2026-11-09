/** Authored civil times remain explicit; recurring clock transitions match the native calendar. */
import { expect, it } from 'vitest';
import { fieldsForTiming, timingPreview, type TimingFields } from '@/features/schedules/timing';
const fields = (change: Partial<TimingFields> = {}): TimingFields => ({ mode: 'once', frequency: 'daily', date: '', time: '', zone: 'UTC', weekdays: [], fold: '', ...change });
const before = Date.parse('2026-01-01T00:00:00Z');
it('does not invent a date or time', () => {
  expect(fieldsForTiming()).toMatchObject({ date: '', time: '', weekdays: [] });
  expect(timingPreview(fields(), before)).toMatchObject({ timing: null, at: null, error: 'missing' });
});
it('converts the chosen zone, including a quarter-hour offset, without using the browser zone', () => {
  expect(timingPreview(fields({ date: '2026-06-10', time: '22:00', zone: 'Asia/Kathmandu' }), before)).toMatchObject({ at: '2026-06-10T16:15:00.000Z', error: null });
});
it.each([{ date: '2026-02-30', time: '09:00', zone: 'UTC' }, { date: '2026-02-01', time: '09:00', zone: 'Not/AZone' }])('rejects invalid civil input %#', (value) => {
  expect(timingPreview(fields(value), before)).toMatchObject({ timing: null, error: 'invalid' });
});
it('refuses a past once instant', () => {
  expect(timingPreview(fields({ date: '2025-12-31', time: '23:59' }), before)).toMatchObject({ timing: null, error: 'future' });
});
it('a nonexistent once minute does not silently move forward', () => {
  expect(timingPreview(fields({ date: '2026-03-08', time: '02:30', zone: 'America/New_York' }), before)).toMatchObject({ timing: null, error: 'invalid' });
});
it('a once fold needs an explicit first/second choice', () => {
  const value = fields({ date: '2026-11-01', time: '01:30', zone: 'America/New_York' });
  expect(timingPreview(value, before)).toMatchObject({ timing: null, ambiguous: true, error: 'ambiguous' });
  expect(timingPreview({ ...value, fold: 'first' }, before).at).toBe('2026-11-01T05:30:00.000Z');
  expect(timingPreview({ ...value, fold: 'second' }, before).at).toBe('2026-11-01T06:30:00.000Z');
});
it('daily gaps preview a skipped occurrence rather than an invented send time', () => {
  expect(timingPreview(fields({ mode: 'repeat', time: '02:30', zone: 'America/New_York' }), Date.parse('2026-03-08T06:00:00Z'))).toMatchObject({ at: null, skipped: '2026-03-08 02:30', error: null });
});
it('after the skipped gap it chooses the next date', () => {
  expect(timingPreview(fields({ mode: 'repeat', time: '02:30', zone: 'America/New_York' }), Date.parse('2026-03-08T07:01:00Z')).at).toBe('2026-03-09T06:30:00.000Z');
});
it('weekly requires an authored weekday and uses ISO Sunday 7', () => {
  expect(timingPreview(fields({ mode: 'repeat', frequency: 'weekly', time: '09:00' }), before).error).toBe('weekdays');
  expect(timingPreview(fields({ mode: 'repeat', frequency: 'weekly', time: '09:00', weekdays: [7] }), before).at).toBe('2026-01-04T09:00:00.000Z');
});
it('a repeat fold uses the first occurrence once and skips the second after it has passed', () => {
  const value = fields({ mode: 'repeat', time: '01:30', zone: 'America/New_York' });
  expect(timingPreview(value, Date.parse('2026-11-01T05:00:00Z')).at).toBe('2026-11-01T05:30:00.000Z');
  expect(timingPreview(value, Date.parse('2026-11-01T05:45:00Z')).at).toBe('2026-11-02T06:30:00.000Z');
});
it('restores the chosen fold even for a stored instant with seconds', () => {
  expect(fieldsForTiming({ kind: 'once', at: '2026-11-01T05:30:34Z', zone: 'America/New_York' })).toMatchObject({ date: '2026-11-01', time: '01:30', fold: 'first' });
});
