/** Performance units and comparisons shared by figures, request rows and model choices. */
import type { TFunction } from 'i18next';
import { providerError } from './outcomes';
import type { RequestTiming, UsageComparison, UsageEvent, UsagePerformance } from '@/generated/api-types';

/** The error denominator contains only completed observations with a known outcome. */
export function errorRate(performance: UsagePerformance | undefined): number | null {
  if (performance === undefined || performance.requests === 0) return null;
  return errorCount(performance) / performance.requests;
}

/** A real zero is retained; every other state has no measured value. */
export function measured(value: RequestTiming['first_token_ms'] | undefined): number | null {
  return value?.state === 'told' ? value.value : null;
}

/** Tier facts alone do not declare token/timing measurements. Retain any other
 * nondefault state or present option, including a measured zero or streamed false. */
export function hasRequestMeasurements(timing: RequestTiming | undefined): boolean {
  if (timing === undefined) return false;
  return Object.entries(timing).some(([field, value]) => {
    if (['wire_protocol','requested_service_tier','reported_service_tier','error_reason','upstream_status','attempt','provider_request_id'].includes(field) || value === null || value === undefined) return false;
    if (field === 'subagent' || field === 'reasoning_hidden') return value !== false;
    return typeof value === 'object' && 'state' in value ? value.state !== 'unsupported' : true;
  });
}

/** Dense legacy/raw counts remain useful when an envelope contains only tier facts.
 * A measured request still keeps each missing token member unknown. */
export function requestTokenCount(row: Pick<UsageEvent, 'timing' | 'scope' | 'tokens'>, output = false): number | null {
  const member = output ? 'output_tokens' : 'input_tokens';
  return row.scope !== 'request' || !hasRequestMeasurements(row.timing)
    ? row.tokens[member]
    : row.timing?.reported_tokens?.[member] ?? null;
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
  if (row.tier_unknown === true || row.tier === null && row.program === null) return null;
  const base = rows.find((other) => other.tier_unknown !== true && other.model === row.model && other.effort === row.effort && other.program === null && other.tier === (row.program === null ? null : row.tier));
  const speed = row.performance.speed_milli, standard = base?.performance.speed_milli;
  return speed === null || standard === null || standard === undefined || standard <= 0 ? null : speed / standard;
}

/** The design's confidence floor is per choice and per measured figure, not its token volume. */
export const CHOICE_SAMPLES = 20;
export function choiceObservation(rows: readonly UsageComparison[], model: string, effort: string | null, tier: string | null, program: string | null) {
  return rows.find((row) => row.tier_unknown !== true && row.model === model && row.effort === effort && row.tier === tier && row.program === program
    && row.performance.speed_samples >= CHOICE_SAMPLES && row.performance.first_token_samples >= CHOICE_SAMPLES
    && row.performance.speed_milli !== null && row.performance.first_token_ms !== null) ?? null;
}

/** Three disjoint input parts; a missing reported part never becomes a measured zero. */
export function requestInputParts(row: Pick<UsageEvent,'scope'|'timing'|'tokens'>) {
  const measured = row.scope === 'request' && hasRequestMeasurements(row.timing);
  const input = measured ? row.timing?.reported_tokens?.input_tokens ?? null : row.tokens.input_tokens;
  const cached = measured ? row.timing?.reported_tokens?.cached_input_tokens ?? null : row.tokens.cached_input_tokens;
  const written = measured ? row.timing?.reported_tokens?.cache_write_tokens ?? null : row.tokens.cache_write_tokens ?? 0;
  return { cached, written, uncached: input === null || cached === null || written === null ? null : Math.max(0,input-cached-written) };
}
