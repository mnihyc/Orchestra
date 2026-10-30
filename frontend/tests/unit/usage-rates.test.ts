/** The rate card as a form says it: what is wrong with a row is said beside its field, a row becomes the rate the API takes, and the card keeps its other rows. */
import { describe, expect, it } from 'vitest';

import type { Rate } from '@/generated/api-types';
import { cardOf, cardWith, cardWithout, check, draftOf, emptyDraft, isDecimal, isScale, rateOf, type RateDraft, stepTokens } from '@/features/usage/rates';

/** A row filled in as a person would, for a test to spoil one part of. */
function filled(over: Partial<RateDraft> = {}): RateDraft {
  return { ...emptyDraft('gpt-6*'), input: '2', cached: '0.1', output: '10', ...over };
}

describe('a rate as a form holds it', () => {
  it('is a number with at most six places, and nothing else', () => {
    for (const good of ['0', '2', '0.1', '10.123456', ' 4 ']) expect(isDecimal(good), good).toBe(true);
    for (const bad of ['', ' ', '-1', '1,5', '1e3', '.5', '10.1234567', 'two']) expect(isDecimal(bad), bad).toBe(false);
  });

  it('is a scale only when it is above nothing', () => {
    expect(isScale('1')).toBe(true);
    expect(isScale('0.0001')).toBe(true);
    expect(isScale('0')).toBe(false);
    expect(isScale('')).toBe(false);
  });

  it('reads the tokens of a step whole or with k and M, and no step of nothing', () => {
    expect(stepTokens('272000')).toBe(272_000);
    expect(stepTokens('272k')).toBe(272_000);
    expect(stepTokens('1M')).toBe(1_000_000);
    expect(stepTokens('0')).toBeNull();
    expect(stepTokens('')).toBeNull();
    expect(stepTokens('a lot')).toBeNull();
  });
});

describe('what is wrong with a row of the card', () => {
  it('is nothing for a row that is whole', () => {
    expect(check(filled(), [], null)).toEqual({});
  });

  it('is said beside each price that is not a number, and for the one that may be left out only when it is written', () => {
    expect(check(filled({ input: '', cached: 'x', output: '-3' }), [], null)).toEqual({
      input: 'usage.rates.faults.number',
      cached: 'usage.rates.faults.number',
      output: 'usage.rates.faults.number',
    });
    expect(check(filled({ cacheWrite: '' }), [], null)).toEqual({});
    expect(check(filled({ cacheWrite: '1,5' }), [], null)).toEqual({ cacheWrite: 'usage.rates.faults.number' });
  });

  it('wants a name in lower case without spaces or a slash, which is not on the card already unless it is the row being changed', () => {
    for (const bad of ['', 'GPT-6', 'gpt 6', 'openai/gpt-6']) expect(check(filled({ pattern: bad }), [], null).pattern, bad).toBe('usage.rates.faults.pattern');
    expect(check(filled({ pattern: 'gpt-6*' }), ['gpt-6*'], null).pattern).toBe('usage.rates.faults.taken');
    expect(check(filled({ pattern: 'gpt-6*' }), ['gpt-6*'], 'gpt-6*')).toEqual({});
    expect(check(filled({ pattern: 'gpt-7' }), ['gpt-6*'], 'gpt-6*')).toEqual({});
  });

  it('refuses a speed with no name, one listed twice and a multiple that is not above nothing', () => {
    const faults = check(filled({ tiers: [{ tier: 'Fast', multiplier: '2' }, { tier: 'batch', multiplier: '0' }, { tier: 'batch', multiplier: '0.5' }] }), [], null);
    expect(faults['tier-0']).toBe('usage.rates.faults.tier');
    expect(faults['multiplier-1']).toBe('usage.rates.faults.multiplier');
    expect(faults['tier-2']).toBe('usage.rates.faults.tierTwice');
    expect(faults['multiplier-2']).toBeUndefined();
  });

  it('refuses a step with no size, two of the same size and a step without its input and its output', () => {
    const step = { tokens: '272k', input: '4', cached: '', output: '15', cacheWrite: '' };
    expect(check(filled({ steps: [step] }), [], null)).toEqual({});
    const faults = check(filled({ steps: [step, { ...step, tokens: '272000' }, { ...step, tokens: '', input: '' }, { ...step, tokens: '1M', output: 'x', cached: 'y' }] }), [], null);
    expect(faults['step-1-tokens']).toBe('usage.rates.faults.tokensTwice');
    expect(faults['step-2-tokens']).toBe('usage.rates.faults.tokens');
    expect(faults['step-2-input']).toBe('usage.rates.faults.number');
    expect(faults['step-3-output']).toBe('usage.rates.faults.number');
    expect(faults['step-3-cached']).toBe('usage.rates.faults.number');
    expect(faults['step-0-tokens']).toBeUndefined();
  });
});

describe('a row of the card as the API takes it', () => {
  it('leaves out what was left empty', () => {
    expect(rateOf(filled({ input: ' 2 ' }))).toEqual({ input_per_mtok: '2', cached_input_per_mtok: '0.1', output_per_mtok: '10' });
  });

  it('carries the cache write, the speeds, and the steps from the smallest prompt up', () => {
    const rate = rateOf(
      filled({
        cacheWrite: '2.5',
        tiers: [{ tier: 'fast', multiplier: '2' }],
        steps: [
          { tokens: '1M', input: '8', cached: '', output: '30', cacheWrite: '' },
          { tokens: '272k', input: '4', cached: '0.2', output: '15', cacheWrite: '5' },
        ],
      }),
    );
    expect(rate).toEqual({
      input_per_mtok: '2',
      cached_input_per_mtok: '0.1',
      output_per_mtok: '10',
      cache_write_per_mtok: '2.5',
      tiers: { fast: '2' },
      above: [
        { context_tokens: 272_000, input_per_mtok: '4', output_per_mtok: '15', cached_input_per_mtok: '0.2', cache_write_per_mtok: '5' },
        { context_tokens: 1_000_000, input_per_mtok: '8', output_per_mtok: '30' },
      ],
    });
  });

  it('comes back as the same row when it is opened to be changed', () => {
    const rate: Rate = {
      input_per_mtok: '2',
      cached_input_per_mtok: '0.1',
      output_per_mtok: '10',
      cache_write_per_mtok: '2.5',
      tiers: { fast: '2', batch: '0.5' },
      above: [{ context_tokens: 272_000, input_per_mtok: '4', output_per_mtok: '15', cached_input_per_mtok: '0.2' }],
    };
    expect(rateOf(draftOf('gpt-6*', rate))).toEqual(rate);
  });
});

describe('the card with a row put in or taken out', () => {
  const other: Rate = { input_per_mtok: '1', cached_input_per_mtok: '0.1', output_per_mtok: '5' };

  it('keeps its other rows', () => {
    const card = cardWith({ 'claude-*': other }, filled(), null);
    expect(Object.keys(card).sort()).toEqual(['claude-*', 'gpt-6*']);
    expect(card['claude-*']).toBe(other);
  });

  it('puts a row changed under another name where it is now, and not where it was', () => {
    const card = cardWith({ 'gpt-6*': other, 'claude-*': other }, filled({ pattern: 'gpt-6-mini' }), 'gpt-6*');
    expect(Object.keys(card).sort()).toEqual(['claude-*', 'gpt-6-mini']);
  });

  it('takes a row out and nothing else', () => {
    expect(Object.keys(cardWithout({ 'gpt-6*': other, 'claude-*': other }, 'gpt-6*'))).toEqual(['claude-*']);
  });

  it('is empty where the setting holds anything but a map', () => {
    expect(cardOf(null)).toEqual({});
    expect(cardOf(['gpt-6*'])).toEqual({});
    expect(cardOf('1')).toEqual({});
    expect(cardOf({ 'gpt-6*': other })).toEqual({ 'gpt-6*': other });
  });
});
