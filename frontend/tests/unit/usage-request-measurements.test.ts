/** Tier-only request envelopes preserve dense counts without inventing measurements or a tier facet. */
import { expect, it } from 'vitest';
import type { RequestTiming } from '@/generated/api-types';
import { hasRequestMeasurements, measured, requestSpeed, requestTokenCount } from '@/features/usage/performance';
import { matchesTier, tierAvailable, tierLabel } from '@/features/usage/tier';
import { i18n } from '@/i18n';
import { timingFixture } from '../doubles/performance';
import { requestRow } from '../doubles/request-row';

function unsupported(changes: Partial<RequestTiming> = {}): RequestTiming {
  return timingFixture({
    asked: null, first_output: null, end: null, reported_tokens: null, output_streamed: null,
    vendor_words: null, error_message: null, subagent: false, reasoning_hidden: false,
    speed_milli: { state: 'unsupported' }, streamed_tokens: { state: 'unsupported' },
    first_token_ms: { state: 'unsupported' }, generation_ms: { state: 'unsupported' },
    request_ms: { state: 'unsupported' }, outcome: { state: 'unsupported' }, ...changes,
  });
}

it('keeps dense raw/legacy token counts for tier-only envelopes without enabling performance', () => {
  const timing = { ...unsupported(), wire_protocol: 'openai-responses', requested_service_tier: 'auto', reported_service_tier: { state: 'told', value: 'default' } as const };
  const row = requestRow({ origin: 'forwarder', timing });
  expect(hasRequestMeasurements(undefined)).toBe(false);
  expect(hasRequestMeasurements(unsupported())).toBe(false);
  expect(hasRequestMeasurements(timing)).toBe(false);
  expect(requestTokenCount(requestRow())).toBe(10);
  expect(requestTokenCount(row)).toBe(10);
  expect(requestTokenCount(row, true)).toBe(5);
  expect(requestSpeed(row.tokens.output_tokens, timing)).toBeNull();
  expect(measured(timing.first_token_ms)).toBeNull();
});

it('keeps declared unknown token members unknown and an actual measured zero visible', () => {
  const timing = unsupported({ first_token_ms: { state: 'unread' } });
  expect(hasRequestMeasurements(timing)).toBe(true);
  expect(requestTokenCount(requestRow({ timing }))).toBeNull();
  expect(requestTokenCount(requestRow({ timing }), true)).toBeNull();
  const zero = unsupported({
    first_token_ms: { state: 'told', value: 0 },
    reported_tokens: { input_tokens: 0, cached_input_tokens: null, cache_write_tokens: null, output_tokens: null, reasoning_tokens: null, total_tokens: null },
  });
  expect(hasRequestMeasurements(zero)).toBe(true);
  expect(measured(zero.first_token_ms)).toBe(0);
  expect(requestTokenCount(requestRow({ timing: zero }))).toBe(0);
  expect(requestTokenCount(requestRow({ timing: zero }), true)).toBeNull();
  expect(requestTokenCount(requestRow({ scope: 'turn', timing: zero }))).toBe(10);
});

it('recognizes nondefault measurement states and present options including streamed false', () => {
  for (const state of ['unread', 'empty'] as const) {
    expect(hasRequestMeasurements(unsupported({ first_token_ms: { state } }))).toBe(true);
  }
  for (const option of [
    { output_streamed: false }, { asked: '2026-11-06T00:00:00Z' },
    { subagent: true }, { reasoning_hidden: true }, { error_message: '' },
    { reported_tokens: { input_tokens: null, cached_input_tokens: null, cache_write_tokens: null, output_tokens: null, reasoning_tokens: null, total_tokens: null } },
  ] satisfies Partial<RequestTiming>[]) expect(hasRequestMeasurements(unsupported(option))).toBe(true);
});

it('hides an unfinished actual tier even without a requested choice or with a prior tier name', () => {
  for (const tier of [null, 'priority']) {
    const row = { tier, timing: { reported_service_tier: { state: 'unread' } } };
    expect(tierAvailable(row)).toBe(false);
    expect(matchesTier(row, { tier: 'none' })).toBe(false);
    expect(matchesTier(row, { tier: 'priority' })).toBe(false);
    expect(matchesTier(row, { tier_unknown: false })).toBe(false);
    expect(matchesTier(row, {})).toBe(true);
  }
});

it('distinguishes unsupported wire capability from historical ordinary rows and an aware missing report', () => {
  const legacy = { tier: null, timing: { reported_service_tier: { state: 'unsupported' } } };
  expect(tierAvailable(legacy)).toBe(true);
  expect(tierLabel(legacy, i18n.t)).toBe(i18n.t('usage.key.standard'));
  expect(matchesTier(legacy, { tier: 'none' })).toBe(true);
  const unsupportedWire = { ...legacy, timing: { ...legacy.timing, wire_protocol: 'wire-without-tier' } };
  expect(tierAvailable(unsupportedWire)).toBe(false);
  expect(matchesTier(unsupportedWire, { tier: 'none' })).toBe(false);
  expect(matchesTier(unsupportedWire, { tier_unknown: true })).toBe(false);
  const missing = { tier: null, tier_unknown: true, timing: { wire_protocol: 'openai-responses', requested_service_tier: 'auto', reported_service_tier: { state: 'empty' } } };
  expect(tierAvailable(missing)).toBe(true);
  expect(tierLabel(missing, i18n.t)).toBe(i18n.t('usage.key.actualTierNotReported'));
  expect(matchesTier(missing, { tier_unknown: true })).toBe(true);
  expect(matchesTier(missing, { tier: 'none' })).toBe(false);
  const reported = { ...missing, tier: 'priority', tier_unknown: false, timing: { ...missing.timing, reported_service_tier: { state: 'told', value: 'priority' } } };
  expect(tierAvailable(reported)).toBe(true);
  expect(matchesTier(reported, { tier: 'priority' })).toBe(true);
});
