/** Performance units and comparisons shared by figures, request rows and model choices. */
import type { TFunction } from 'i18next';
import { providerError } from './outcomes';
import type { RequestTiming, UsageComparison, UsagePerformance } from '@/generated/api-types';

/** The error denominator contains only completed observations with a known outcome. */
export function errorRate(performance: UsagePerformance | undefined): number | null {
  if (performance === undefined || performance.requests === 0) return null;
  return errorCount(performance) / performance.requests;
}

/** A real zero is retained; every other state has no measured value. */
export function measured(value: RequestTiming['first_token_ms'] | undefined): number | null {
  return value?.state === 'told' ? value.value : null;
}

/** Eligibility and hidden-reasoning arithmetic belong to the shared server module. */
export function requestSpeed(_output: number, timing: RequestTiming | undefined): number | null {
  const speed = measured(timing?.speed_milli);
  return speed === null ? null : speed / 1000;
}

export function speedText(value: number | null | undefined, locale: string, t: TFunction): string {
  return value === null || value === undefined || !Number.isFinite(value) ? t('usagePerf.dash')
    : t('usagePerf.speedValue', { value: new Intl.NumberFormat(locale, { minimumFractionDigits: value < 10 ? 1 : 0, maximumFractionDigits: value < 10 ? 1 : 0 }).format(value) });
}

export function timeText(ms: number | null | undefined, locale: string, t: TFunction): string {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return t('usagePerf.dash');
  const seconds = ms / 1000;
  const number = (n: number, digits = 0) => new Intl.NumberFormat(locale, { minimumFractionDigits: digits, maximumFractionDigits: digits }).format(n);
  if (seconds >= 60) {
    const rounded = Math.round(seconds);
    return t('usagePerf.minutesSeconds', { minutes: number(Math.floor(rounded / 60)), seconds: number(rounded % 60) });
  }
  return t('usagePerf.timeValue', { value: number(seconds, seconds < 10 ? 1 : 0) });
}

export function rateText(rate: number | null, locale: string, t: TFunction): string {
  return rate === null || !Number.isFinite(rate) ? t('usagePerf.dash') : new Intl.NumberFormat(locale, { style: 'percent', minimumFractionDigits: 1, maximumFractionDigits: 1 }).format(rate);
}

export const errorCount = (performance: UsagePerformance | undefined): number => Object.entries(performance?.errors ?? {}).filter(([code]) => providerError(code)).reduce((sum, [,n]) => sum + n, 0);

export function errorsText(performance: UsagePerformance | undefined, locale: string, t: TFunction): string {
  const rate=errorRate(performance);
  return rate===null ? t('usagePerf.dash') : t('usagePerf.rateCount', {rate:rateText(rate,locale,t),errors:new Intl.NumberFormat(locale).format(errorCount(performance))});
}

/** Compare like model/effort choices; a programme is compared with the same tier without it. */
export function relativeSpeed(row: UsageComparison, rows: readonly UsageComparison[]): number | null {
  if (row.tier === null && row.program === null) return null;
  const base = rows.find((other) => other.model === row.model && other.effort === row.effort && other.program === null && other.tier === (row.program === null ? null : row.tier));
  const speed = row.performance.speed_milli, standard = base?.performance.speed_milli;
  return speed === null || standard === null || standard === undefined || standard <= 0 ? null : speed / standard;
}

/** The design's confidence floor is per choice and per measured figure, not its token volume. */
export const CHOICE_SAMPLES = 20;
export function choiceObservation(rows: readonly UsageComparison[], model: string, effort: string | null, tier: string | null, program: string | null) {
  return rows.find((row) => row.model === model && row.effort === effort && row.tier === tier && row.program === program
    && row.performance.speed_samples >= CHOICE_SAMPLES && row.performance.first_token_samples >= CHOICE_SAMPLES
    && row.performance.speed_milli !== null && row.performance.first_token_ms !== null) ?? null;
}
