/** Unknown actual tier stays distinct through usage navigation and measured comparisons. */
import { expect, it } from 'vitest';
import type { UsageComparison } from '@/generated/api-types';
import { filterValue, isRest, keyId, keyLabel } from '@/features/usage/keys';
import { choiceObservation, relativeSpeed } from '@/features/usage/performance';
import { activeFilters, filtersOf, patchSearch, readSearch } from '@/features/usage/search';
import { matchesTier, tierAvailable, tierFilter, tierId, tierLabel } from '@/features/usage/tier';
import { i18n } from '@/i18n';
import { performanceOf, timingFixture } from '../doubles/performance';

const unknown = { tier: null, tier_unknown: true };
const ordinary = { tier: null };
const unavailable = {
  tier: null,
  timing: { requested_service_tier: 'priority', reported_service_tier: { state: 'unsupported' } },
};

it('preserves historical Standard and reported vendor strings while naming actual absence explicitly', () => {
  expect(tierLabel(ordinary, i18n.t)).toBe('Standard');
  expect(tierLabel({ tier: 'vendor-specific-speed' }, i18n.t)).toBe('vendor-specific-speed');
  expect(tierLabel(unknown, i18n.t)).toBe('Actual tier not reported');
  expect(keyLabel('tier', unknown, i18n.t)).toBe('Actual tier not reported');
  expect(isRest({ tier_unknown: true })).toBe(false);
  expect(isRest({ tier_unknown: false })).toBe(true);
  expect(filterValue('tier', unknown)).toBeNull();
  expect(new Set([keyId('tier', ordinary), keyId('tier', unknown), keyId('tier', { tier: '[null,true]' })]).size).toBe(3);
});

it('drills an unknown bucket with a boolean and never the ordinary-tier query', () => {
  const search = patchSearch({ tier: 'priority', model: 'route' }, tierFilter(unknown));
  expect(search).toEqual({ model: 'route', tier_unknown: true });
  expect(filtersOf(search)).toEqual({ model: 'route', tier_unknown: true });
  expect(activeFilters(search)).toContainEqual({ name: 'tier_unknown', value: true });
  expect(readSearch({ tier: 'none', tier_unknown: 'true' })).toEqual({ tier_unknown: true });
  expect(JSON.parse(tierId(unknown))).toEqual([null, true]);
});

it('switches and clears tier selections without retaining another tier constraint', () => {
  const unknownSearch = readSearch({ tier_unknown: 'true', effort: 'high' });
  expect(patchSearch(unknownSearch, tierFilter({ tier: 'priority' }))).toEqual({ effort: 'high', tier: 'priority' });
  expect(patchSearch(unknownSearch, tierFilter(ordinary))).toEqual({ effort: 'high', tier: 'none' });
  expect(patchSearch(unknownSearch, { tier_unknown: undefined })).toEqual({ effort: 'high' });
  expect(filtersOf(readSearch({ tier_unknown: 'false' }))).toEqual({ tier_unknown: false });
  expect(readSearch({ tier_unknown: 'not-a-boolean' })).toEqual({});
});

it('hides unsupported explicit-tier facts without reclassifying historical ordinary requests', () => {
  expect(tierAvailable(unavailable)).toBe(false);
  expect(tierAvailable({ ...unavailable, timing: { ...unavailable.timing, reported_service_tier: { state: 'empty' } } })).toBe(true);
  expect(tierAvailable({ ...unavailable, timing: { ...unavailable.timing, reported_service_tier: { state: 'told' } } })).toBe(true);
  expect(tierAvailable(ordinary)).toBe(true);
  expect(tierAvailable({ ...unavailable, timing: { ...unavailable.timing, requested_service_tier: 'standard' } })).toBe(true);
});

it('keeps local live-log tier matching consistent with the optional HTTP filters', () => {
  for (const row of [ordinary, unknown, unavailable]) expect(matchesTier(row, {})).toBe(true);
  expect(matchesTier(ordinary, { tier: 'none' })).toBe(true);
  expect(matchesTier(unknown, { tier: 'none' })).toBe(false);
  expect(matchesTier(unavailable, { tier: 'none' })).toBe(false);
  expect(matchesTier(unknown, { tier_unknown: true })).toBe(true);
  expect(matchesTier(ordinary, { tier_unknown: true })).toBe(false);
  expect(matchesTier(ordinary, { tier_unknown: false })).toBe(true);
  expect(matchesTier(unknown, { tier_unknown: false })).toBe(false);
  expect(matchesTier(unavailable, { tier_unknown: false })).toBe(false);
  expect(matchesTier({ tier: 'vendor-specific-speed' }, { tier: 'vendor-specific-speed' })).toBe(true);
});

it('never uses unknown actual-tier measurements as a Standard choice or speed baseline', () => {
  const measured = { ...performanceOf([timingFixture()]), speed_samples: 20, first_token_samples: 20, speed_milli: 50_000, first_token_ms: 1000 };
  const unknownRow: UsageComparison = { model: 'route', effort: null, tier: null, tier_unknown: true, program: null, performance: { ...measured, speed_milli: 1_000_000 } };
  const standard: UsageComparison = { model: 'route', effort: null, tier: null, program: null, performance: measured };
  const paid: UsageComparison = { ...standard, tier: 'priority', performance: { ...measured, speed_milli: 100_000 } };
  expect(choiceObservation([unknownRow], 'route', null, null, null)).toBeNull();
  expect(choiceObservation([unknownRow, standard], 'route', null, null, null)).toBe(standard);
  expect(relativeSpeed(paid, [unknownRow, standard, paid])).toBe(2);
  expect(relativeSpeed(paid, [unknownRow, paid])).toBeNull();
  expect(relativeSpeed(unknownRow, [unknownRow, standard])).toBeNull();
});
