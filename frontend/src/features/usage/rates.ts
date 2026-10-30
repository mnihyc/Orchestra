/** The rate card of a place as a form holds it and as the API says it: a row of the card in texts, what is wrong with a row, and the card with a row put in or taken out.
 *
 * Pure functions, so that "a rate that is not a number is refused beside its field" and "the card keeps
 * its other rows" can be tested without drawing a dialog. A rate is credits for a million tokens, a
 * decimal string with at most six places; a card is a pattern that names models by their routing names
 * (lower case, `*` for any run of characters) for each rate.
 */
import type { Rate, RateAbove } from '@/generated/api-types';
import { amountText } from '@/features/usage/capForm';

/** The rate card of a place: what it says each model costs. */
export type Card = Record<string, Rate>;

/** The most places a rate has after the point. */
export const RATE_PLACES = 6;

const DECIMAL = new RegExp(`^\\d+(\\.\\d{1,${RATE_PLACES}})?$`);
const TIER = /^[a-z0-9][a-z0-9_-]*$/;

/** Whether a text is a rate: digits, and places after a point. */
export const isDecimal = (text: string): boolean => DECIMAL.test(text.trim());

/** One step for large prompts, in texts. */
export interface StepDraft {
  tokens: string;
  input: string;
  cached: string;
  output: string;
  cacheWrite: string;
}

/** One speed and what it multiplies the rate by, in texts. */
export interface TierDraft {
  tier: string;
  multiplier: string;
}

/** A row of the card, in texts. */
export interface RateDraft {
  pattern: string;
  input: string;
  cached: string;
  output: string;
  cacheWrite: string;
  tiers: TierDraft[];
  steps: StepDraft[];
}

export const emptyStep = (): StepDraft => ({ tokens: '', input: '', cached: '', output: '', cacheWrite: '' });

/** A row with nothing in it, for a pattern. */
export function emptyDraft(pattern = ''): RateDraft {
  return { pattern, input: '', cached: '', output: '', cacheWrite: '', tiers: [], steps: [] };
}

/** The row of an existing rate, to be changed. */
export function draftOf(pattern: string, rate: Rate): RateDraft {
  return {
    pattern,
    input: rate.input_per_mtok,
    cached: rate.cached_input_per_mtok,
    output: rate.output_per_mtok,
    cacheWrite: rate.cache_write_per_mtok ?? '',
    tiers: Object.entries(rate.tiers ?? {}).map(([tier, multiplier]) => ({ tier, multiplier })),
    steps: (rate.above ?? []).map((step) => ({
      tokens: String(step.context_tokens),
      input: step.input_per_mtok,
      cached: step.cached_input_per_mtok ?? '',
      output: step.output_per_mtok,
      cacheWrite: step.cache_write_per_mtok ?? '',
    })),
  };
}

/** What is wrong with one field, as the key of the sentence that says it. */
export type Fault =
  | 'usage.rates.faults.pattern'
  | 'usage.rates.faults.taken'
  | 'usage.rates.faults.number'
  | 'usage.rates.faults.multiplier'
  | 'usage.rates.faults.tier'
  | 'usage.rates.faults.tierTwice'
  | 'usage.rates.faults.tokens'
  | 'usage.rates.faults.tokensTwice';

/** What is wrong with a row, by the field it is beside: `pattern`, `input`, `cached`, `output`, `cacheWrite`, `tier-<n>`, `multiplier-<n>`, `step-<n>-<field>`. */
export type Faults = Partial<Record<string, Fault>>;

/** The tokens of a step, written whole or with k and M: the number, or none where it is not one. */
export const stepTokens = (text: string): number | null => {
  const whole = amountText(text, 'tokens');
  return whole === null || Number(whole) < 1 ? null : Number(whole);
};

/**
 * What is wrong with a row of the card. `taken` is the patterns the card has already; the one being
 * changed (`replacing`) is not taken by itself.
 */
export function check(draft: RateDraft, taken: readonly string[], replacing: string | null): Faults {
  const faults: Faults = {};
  const pattern = draft.pattern.trim();
  if (pattern === '' || /\s|\//.test(pattern) || pattern !== pattern.toLowerCase()) faults.pattern = 'usage.rates.faults.pattern';
  else if (pattern !== replacing && taken.includes(pattern)) faults.pattern = 'usage.rates.faults.taken';

  const required = (field: string, text: string) => {
    if (!isDecimal(text)) faults[field] = 'usage.rates.faults.number';
  };
  const optional = (field: string, text: string) => {
    if (text.trim() !== '' && !isDecimal(text)) faults[field] = 'usage.rates.faults.number';
  };
  required('input', draft.input);
  required('cached', draft.cached);
  required('output', draft.output);
  optional('cacheWrite', draft.cacheWrite);

  const seenTiers = new Set<string>();
  draft.tiers.forEach((one, at) => {
    const name = one.tier.trim();
    if (!TIER.test(name)) faults[`tier-${at}`] = 'usage.rates.faults.tier';
    else if (seenTiers.has(name)) faults[`tier-${at}`] = 'usage.rates.faults.tierTwice';
    seenTiers.add(name);
    if (!isDecimal(one.multiplier) || Number(one.multiplier) <= 0) faults[`multiplier-${at}`] = 'usage.rates.faults.multiplier';
  });

  const seenSizes = new Set<number>();
  draft.steps.forEach((step, at) => {
    const size = stepTokens(step.tokens);
    if (size === null) faults[`step-${at}-tokens`] = 'usage.rates.faults.tokens';
    else if (seenSizes.has(size)) faults[`step-${at}-tokens`] = 'usage.rates.faults.tokensTwice';
    else seenSizes.add(size);
    required(`step-${at}-input`, step.input);
    required(`step-${at}-output`, step.output);
    optional(`step-${at}-cached`, step.cached);
    optional(`step-${at}-cacheWrite`, step.cacheWrite);
  });
  return faults;
}

/** The rate a checked row says, as the API takes it: what is left empty is left out, and the steps go from the smallest prompt up. */
export function rateOf(draft: RateDraft): Rate {
  const text = (value: string) => value.trim();
  const steps: RateAbove[] = draft.steps
    .map((step) => ({
      context_tokens: stepTokens(step.tokens) ?? 0,
      input_per_mtok: text(step.input),
      output_per_mtok: text(step.output),
      ...(text(step.cached) === '' ? {} : { cached_input_per_mtok: text(step.cached) }),
      ...(text(step.cacheWrite) === '' ? {} : { cache_write_per_mtok: text(step.cacheWrite) }),
    }))
    .sort((one, other) => one.context_tokens - other.context_tokens);
  return {
    input_per_mtok: text(draft.input),
    cached_input_per_mtok: text(draft.cached),
    output_per_mtok: text(draft.output),
    ...(text(draft.cacheWrite) === '' ? {} : { cache_write_per_mtok: text(draft.cacheWrite) }),
    ...(draft.tiers.length === 0 ? {} : { tiers: Object.fromEntries(draft.tiers.map((one) => [text(one.tier), text(one.multiplier)])) }),
    ...(steps.length === 0 ? {} : { above: steps }),
  };
}

/** The card with a row put in; a row that was changed under another pattern leaves its old one. */
export function cardWith(card: Card, draft: RateDraft, replacing: string | null): Card {
  const pattern = draft.pattern.trim();
  const kept = Object.entries(card).filter(([one]) => one !== replacing && one !== pattern);
  return { ...Object.fromEntries(kept), [pattern]: rateOf(draft) };
}

/** The card without a row. */
export function cardWithout(card: Card, pattern: string): Card {
  return Object.fromEntries(Object.entries(card).filter(([one]) => one !== pattern));
}

/** The card a setting holds: its rows, or none where it holds something else. */
export function cardOf(value: unknown): Card {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) return {};
  return value as Card;
}

/** Whether a text is a scale: a number above nothing. */
export const isScale = (text: string): boolean => isDecimal(text) && Number(text) > 0;
