/** A row too narrow for a credits column says what its cell says: use with no rate is never a bare dash or a known zero. */
import { expect, it } from 'vitest';
import type { UsageFigures } from '@/generated/api-types';
import { creditsLine } from '@/features/usage/Amounts';
import { i18n } from '@/i18n';

const figures = (more: Partial<UsageFigures>): UsageFigures => ({
  turns: 30,
  tokens: { input_tokens: 1000, cached_input_tokens: 0, cache_write_tokens: 0, output_tokens: 500, reasoning_tokens: 0, total_tokens: 18_000_000 },
  credits: '0', estimated_credits: '0', unrated_tokens: 0,
  ...more,
});

it('says the tokens left without a rate beside the dash of use that had none', () => {
  const line = creditsLine(figures({ unrated_tokens: 18_000_000 }), i18n.t, 'en');
  expect(line).toBe(`– · ${i18n.t('usage.amounts.unratedOnly', { tokens: '18M' })}`);
  expect(line).toContain('18M tokens without a rate');
});

it('marks an estimate and says what a sum leaves out', () => {
  expect(creditsLine(figures({ credits: '12', estimated_credits: '12' }), i18n.t, 'en')).toBe('≈ 12.0');
  const partly = creditsLine(figures({ credits: '12', estimated_credits: '3', unrated_tokens: 600_000 }), i18n.t, 'en');
  expect(partly).toContain('12.0');
  expect(partly).toContain(i18n.t('usage.amounts.estimatedPart', { amount: '3' }));
  expect(partly).toContain('600k tokens without a rate');
});

it('reads a plain sum as its figure alone', () => {
  expect(creditsLine(figures({ credits: '151' }), i18n.t, 'en')).toBe('151.0');
});
