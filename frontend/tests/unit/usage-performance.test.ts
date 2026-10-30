/** The visible units, confidence floor and non-additive chart series preserve measured and missing values. */
import { expect, it } from 'vitest';
import type { UsageAnalytics, UsageComparison, UsageFigures } from '@/generated/api-types';
import { choiceObservation, errorRate, measured, speedText, timeText } from '@/features/usage/performance';
import { barsOf } from '@/features/usage/series';
import { i18n } from '@/i18n';
import { performanceOf, timingFixture } from '../doubles/performance';

it('formats sensible speed and time units and preserves a measured zero', () => {
  expect(speedText(85.2, 'en', i18n.t)).toBe('85 tok/s');
  expect(speedText(9.24, 'en', i18n.t)).toBe('9.2 tok/s');
  expect(speedText(null, 'en', i18n.t)).toBe('—');
  expect(timeText(1200, 'en', i18n.t)).toBe('1.2 s');
  expect(timeText(12_200, 'en', i18n.t)).toBe('12 s');
  expect(timeText(72_000, 'en', i18n.t)).toBe('1 min 12 s');
  expect(measured({ state: 'told', value: 0 })).toBe(0);
  expect(measured({ state: 'unsupported' })).toBeNull();
});

it('offers a hint only for the exact model, effort, tier and programme with twenty measured requests', () => {
  const performance = { ...performanceOf([timingFixture()]), speed_samples: 20, first_token_samples: 20 };
  const row: UsageComparison = { model: 'Swift', effort: 'high', tier: 'fast', program: null, performance };
  expect(choiceObservation([row], 'Swift', 'high', 'fast', null)).toBe(row);
  expect(choiceObservation([{ ...row, performance: { ...performance, speed_samples: 19 } }], 'Swift', 'high', 'fast', null)).toBeNull();
  expect(choiceObservation([row], 'Swift', 'low', 'fast', null)).toBeNull();
  expect(choiceObservation([row], 'Swift', 'high', null, null)).toBeNull();
  expect(choiceObservation([row], 'Swift', 'high', 'fast', 'research')).toBeNull();
});

it('keeps medians and p90 separate rather than adding medians across days, and cancellations out of errors', () => {
  const one = timingFixture(), two = timingFixture({ speed_milli: { state: 'told', value: 128_000 } });
  const figures = (timings: typeof one[]): UsageFigures => ({ turns: 1, credits: '0', estimated_credits: '0', unrated_tokens: 0, tokens: { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 }, performance: performanceOf(timings) });
  const analytics: UsageAnalytics = { scope: 'me', depth: 'own', from: '2026-10-14', to: '2026-10-15', bucket: 'day', me: null, totals: figures([one, two]), breakdowns: [], series: [one, two].map((timing, index) => ({ at: `2026-10-${14 + index}T00:00:00Z`, figures: figures([timing]), by: [] })) };
  const series = barsOf(analytics, 'speed', 'none', i18n.t);
  expect(series.lines).toBe(true);
  expect(series.stacks[0]?.values).toEqual([61.287, 122.573]);
  expect(series.stacks[0]?.total).toBe(61.287);
  expect(series.stacks[0]?.highs).toEqual([61.287, 122.573]);
  const p = performanceOf([timingFixture({ outcome: { state: 'told', value: 'forced' } }), timingFixture({ outcome: { state: 'told', value: 'provider_busy' } })]);
  expect(errorRate(p)).toBe(0.5);
  expect(p.errors).toEqual({ provider_busy: 1 });
  const otherFailures=performanceOf(['harness_error','session_lost','environment_unreachable'].map((value) => timingFixture({ outcome: { state: 'told', value } })));
  expect(otherFailures.requests).toBe(3);
  expect(errorRate(otherFailures)).toBe(0);
  expect(otherFailures.errors).toEqual({});
});
