/** The time axis of the usage chart says which day its hours are on, and a lone hour among many days can be seen. */
import { expect, it } from 'vitest';
import { BAR_NARROWEST_PX, BAR_SIZE, dividerLeftOut, timeWords } from '@/features/usage/Chart';

const DAY_SECONDS = 86_400;
const WEEK_MS = 7 * DAY_SECONDS * 1000;

it.each([['en', 'Nov 17', '6:00 AM'], ['zh-CN', '11月17日', '6:00']] as const)('names the day where an hour of several days starts one, in %s', (locale, day, hour) => {
  const words = timeWords(locale, 'hour', WEEK_MS);
  const midnight = Date.parse('2026-11-17T00:00:00Z') / 1000;
  expect(words.tick(midnight)).toBe(day);
  expect(words.tick(midnight + 6 * 3600)).toBe(hour);
  // Eight midnights of a week read as eight days, never "12:00 AM" eight times.
  const labels = Array.from({ length: 8 }, (_, at) => words.tick(midnight + at * DAY_SECONDS));
  expect(new Set(labels).size).toBe(8);
});

it('keeps an hour bar wide enough to be seen when a week of hours shares a phone-wide chart', () => {
  // 168 hours over about 300 pixels leave under two pixels each: the bar is drawn at least this wide.
  expect(BAR_NARROWEST_PX).toBeGreaterThanOrEqual(3);
  expect(BAR_SIZE[2]).toBe(BAR_NARROWEST_PX);
});

it('knows where uPlot leaves out the line between stacks, so that a bar there keeps its own colour', () => {
  // Buckets one hour apart over a plot `width` pixels wide, as uPlot measures them in canvas pixels.
  const chart = (count: number, width: number) => {
    const xs = Array.from({ length: count }, (_, at) => at * 3600);
    return { data: [xs] as unknown as [number[]], valToPos: (value: number) => value / (count * 3600) * width };
  };
  // A week of hours on a phone (about 1.7 px a bucket): the line is left out, at both pixel ratios.
  expect(dividerLeftOut(chart(168, 290), 1)).toBe(true);
  expect(dividerLeftOut(chart(168, 580), 2)).toBe(true);
  // The same week at 1440 (about 3.3 px a bucket) keeps it; seven days by the day keep it on a phone.
  expect(dividerLeftOut(chart(168, 560), 1)).toBe(false);
  expect(dividerLeftOut(chart(7, 290), 1)).toBe(false);
  expect(dividerLeftOut(chart(1, 290), 1)).toBe(false);
});
