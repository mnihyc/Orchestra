/** How the figures of usage are written and compared: credits with the digits that mean something, what is an estimate and what had no rate, the change against the span before, and the share of the tokens that a rate divides them into.
 *
 * Pure functions, so that every rule the specification gives about a figure can be tested without a screen.
 * A credit is a decimal the server adds exactly; it is read as a number here only to be written down, and
 * what is written is never more exact than the sum it came from.
 */
import type { TFunction } from 'i18next';
import type { TokenUsage, UsageFigures } from '@/generated/api-types';
import { formatNumber, formatTokens } from '@/lib/format';
import { i18n } from '@/i18n';
import { errorRate, speedText, timeText, rateText } from '@/features/usage/performance';

import type { Measure } from '@/features/usage/search';

/** The mark that stands before a figure that is an estimate. */
export const ESTIMATE_MARK = '≈';

/** A decimal string as a number; nothing for text that is none. */
export function decimal(value: string | null | undefined): number {
  if (value === null || value === undefined) return 0;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

/** Below this an amount of credits is written as "less than" it: a thousandth of a credit is no figure to read. */
const CREDITS_LEAST_SHOWN = 0.001;
/** From this an amount of credits is written with one decimal, and below it with the two that tell a cheap turn from a free one. */
const CREDITS_ONE_DECIMAL_FROM = 10;

/** An amount of credits in the language of the reader: one decimal from ten, two below, and the digits that count for a fraction. */
export function formatCredits(value: number, locale: string): string {
  if (value === 0) return formatNumber(0, locale);
  if (value > 0 && value < CREDITS_LEAST_SHOWN) return `<${formatNumber(CREDITS_LEAST_SHOWN, locale)}`;
  if (value >= CREDITS_ONE_DECIMAL_FROM) return formatNumber(value, locale, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  if (value >= 1) return formatNumber(value, locale, { maximumFractionDigits: 2 });
  return formatNumber(value, locale, { maximumSignificantDigits: 2 });
}

/** The figures of a span read for a measure the chart or a table is about. */
export function measureOf(figures: UsageFigures, measure: Measure): number {
  switch (measure) {
    case 'credits':
      return decimal(figures.credits);
    case 'tokens':
      return figures.tokens.total_tokens;
    case 'turns':
      return figures.turns;
    case 'requests':
      return figures.performance?.requests ?? 0;
    case 'speed': return figures.performance?.speed_milli === null || figures.performance?.speed_milli === undefined ? Number.NaN : figures.performance.speed_milli / 1000;
    case 'first_token': return figures.performance?.first_token_ms ?? Number.NaN;
    case 'errors': return errorRate(figures.performance) ?? Number.NaN;
  }
}

/** A value of a measure as it is written beside the chart and in its readout. */
export function formatMeasure(value: number, measure: Measure, locale: string): string {
  if (!Number.isFinite(value)) return i18n.t('usagePerf.dash');
  if (measure === 'speed') return speedText(value, locale, i18n.t);
  if (measure === 'first_token') return timeText(value, locale, i18n.t);
  if (measure === 'errors') return rateText(value, locale, i18n.t);
  return measure === 'credits' ? formatCredits(value, locale) : measure === 'tokens' ? formatTokens(value, locale) : formatNumber(value, locale);
}

/** A value of a measure on the scale of a chart: shorter, because many are written side by side. */
export function formatScale(value: number, measure: Measure, locale: string): string {
  if (measure === 'speed' || measure === 'first_token' || measure === 'errors') return formatMeasure(value, measure, locale);
  if (measure === 'tokens') return formatTokens(value, locale);
  return formatNumber(value, locale, { maximumFractionDigits: value < 10 ? 1 : 0 });
}

// ---------------------------------------------------------------- what a sum of credits says of itself

/** What is known of the credits of a sum, beside the sum. */
export interface CreditsState {
  /** The sum, over the use that had a rate. */
  sum: number;
  /** The part of it counted at the shipped catalogue's rates: an estimate that nobody here set. */
  estimated: number;
  /** The tokens of use that had no rate at all, which add nothing to the sum. */
  unrated: number;
  /** All of the use had a rate that is an estimate: the sum itself reads as one. */
  allEstimated: boolean;
  /** None of the use had a rate: the credits read "–", never 0. */
  none: boolean;
}

/** Whether a sum is the sum of nothing that had a rate: something was used and none of it could be rated. */
export function creditsState(figures: UsageFigures): CreditsState {
  const sum = decimal(figures.credits);
  const estimated = decimal(figures.estimated_credits);
  const unrated = figures.unrated_tokens;
  const used = figures.tokens.total_tokens > 0 || figures.turns > 0;
  return {
    sum,
    estimated,
    unrated,
    allEstimated: sum > 0 && estimated >= sum,
    none: used && sum === 0 && unrated > 0,
  };
}

// ---------------------------------------------------------------- the change against the span before

/** How a figure stands against the same figure of the span before it. */
export type Change = { kind: 'none' } | { kind: 'new' } | { kind: 'same' } | { kind: 'up'; percent: number } | { kind: 'down'; percent: number };

/** Below this share of change two spans are the same as far as a reader is concerned. */
const CHANGE_SAME_BELOW_PERCENT = 0.5;

/** A figure against the one before; none where there is no span before, `new` where there was nothing before and something now. */
export function changeOf(now: number, before: number | null): Change {
  if (before === null) return { kind: 'none' };
  if (before === 0) return now === 0 ? { kind: 'same' } : { kind: 'new' };
  const percent = ((now - before) / before) * 100;
  if (Math.abs(percent) < CHANGE_SAME_BELOW_PERCENT) return { kind: 'same' };
  return percent > 0 ? { kind: 'up', percent: Math.round(percent) } : { kind: 'down', percent: Math.round(-percent) };
}

// ---------------------------------------------------------------- tokens the way a rate divides them

/** The tokens of a span as a rate divides them: input that was not cached, cached input, and output with the part of it that was reasoning. */
export interface TokenParts {
  input: number;
  cached: number;
  output: number;
  reasoning: number;
}

export function tokenParts(tokens: TokenUsage): TokenParts {
  return {
    // Cached input is a part of input, so the input that was not cached is what is left of it.
    input: Math.max(0, tokens.input_tokens - tokens.cached_input_tokens),
    cached: tokens.cached_input_tokens,
    output: tokens.output_tokens,
    reasoning: tokens.reasoning_tokens,
  };
}

/** Whether a span used anything at all: a span that did not is said to be empty and draws no chart. */
export function isEmpty(figures: UsageFigures): boolean {
  return figures.turns === 0 && figures.tokens.total_tokens === 0 && decimal(figures.credits) === 0 && (figures.performance?.requests ?? 0) === 0;
}

/** An empty metric is not an empty span: unmetered turns and unpriced tokens still happened. */
export function chartEmptyText(figures: UsageFigures, measure: Measure, t: TFunction): string {
  if (isEmpty(figures)) return t('usage.chart.empty');
  if (measure !== 'credits' && measure !== 'tokens' && measure !== 'turns' && measure !== 'requests') return t('usagePerf.notMeasured');
  if (measureOf(figures, measure) > 0) return t('usage.chart.noReadings');
  if (measure === 'credits' && creditsState(figures).none) return t('usage.chart.unrated');
  const keys = { credits: 'usage.chart.zeroCredits', tokens: 'usage.chart.zeroTokens', turns: 'usage.chart.zeroTurns', requests: 'usage.chart.zeroRequests' } as const;
  return t(keys[measure]);
}
