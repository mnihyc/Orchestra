/** Exact ledger addition and UTC daily period selection without vendor counters. */
import type { UsageFigures } from '@/generated/api-types';
import type { PlanAnalytics } from './plan';
import { ledgerPeriods } from './plan';

/** Ledger credits have at most six decimal places; integer arithmetic preserves small rated amounts. */
const CREDIT_DECIMALS = 6;
const CREDIT_SCALE = 10n ** BigInt(CREDIT_DECIMALS);
const zero = (): UsageFigures => ({ turns: 0, tokens: { input_tokens: 0, cached_input_tokens: 0, output_tokens: 0, reasoning_tokens: 0, total_tokens: 0 }, credits: '0', estimated_credits: '0', unrated_tokens: 0 });

export function sumCredits(values: string[]): string {
  const total = values.reduce((sum, value) => {
    const negative = value.startsWith('-');
    const [whole = '0', fraction = ''] = value.replace(/^-/, '').split('.');
    const scaled = BigInt(whole) * CREDIT_SCALE + BigInt(fraction.padEnd(CREDIT_DECIMALS, '0'));
    return sum + (negative ? -scaled : scaled);
  }, 0n);
  const absolute = total < 0n ? -total : total;
  const fraction = String(absolute % CREDIT_SCALE).padStart(CREDIT_DECIMALS, '0').replace(/0+$/, '');
  return `${total < 0n ? '-' : ''}${absolute / CREDIT_SCALE}${fraction === '' ? '' : `.${fraction}`}`;
}

export function sumFigures(figures: UsageFigures[]): UsageFigures {
  return figures.reduce((sum, row) => ({ turns: sum.turns + row.turns,
    tokens: { input_tokens: sum.tokens.input_tokens + row.tokens.input_tokens, cached_input_tokens: sum.tokens.cached_input_tokens + row.tokens.cached_input_tokens, output_tokens: sum.tokens.output_tokens + row.tokens.output_tokens, reasoning_tokens: sum.tokens.reasoning_tokens + row.tokens.reasoning_tokens, total_tokens: sum.tokens.total_tokens + row.tokens.total_tokens },
    credits: sumCredits([sum.credits, row.credits]), estimated_credits: sumCredits([sum.estimated_credits, row.estimated_credits]), unrated_tokens: sum.unrated_tokens + row.unrated_tokens,
  }), zero());
}

export function periodFigures(analytics: PlanAnalytics, now: number): { today: UsageFigures; sevenDays: UsageFigures; month: UsageFigures } {
  const periods = ledgerPeriods(now);
  const after = (from: number) => sumFigures(analytics.series.filter((row) => Date.parse(row.at) >= from && Date.parse(row.at) < Date.parse(`${periods.to}T00:00:00Z`)).map((row) => row.figures));
  return { today: after(periods.today), sevenDays: after(periods.sevenDays), month: after(periods.month) };
}

