/** The same observed-pace sentence below plan windows and period caps. */
import { useTranslation } from 'react-i18next';
import type { UsageForecast } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { DAY_MS, HOUR_MS, MINUTE_MS, formatDateTime, formatSpan, formatTimeOfDay, useNow, type TimeContext } from '@/lib/time';
import { cx } from '@/ui/cx';

/** Two missed hourly idle reads make the pace stale. */
export const PLAN_READING_MAX_AGE = 2 * HOUR_MS;
/** Estimates use five-minute steps within a day, hours beyond it. */
const FORECAST_NEAR_ROUNDING = 5 * MINUTE_MS;

export function forecastTime(at: number, now: number, format: TimeContext): string {
  const near = Math.abs(at - now) <= DAY_MS;
  const step = near ? FORECAST_NEAR_ROUNDING : HOUR_MS;
  const rounded = Math.round(at / step) * step;
  return near ? formatTimeOfDay(rounded,format,now) : formatDateTime(rounded,format,now);
}

export interface ForecastLineProps {
  forecast?: UsageForecast | null | undefined;
  usedPercent: number;
  resetsAt: string | null;
  lastAt?: string | null;
  cap?: boolean;
}

export function ForecastLine({ forecast, usedPercent, resetsAt, lastAt, cap = false }: ForecastLineProps) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const now = useNow();
  const reset = resetsAt === null ? null : Date.parse(resetsAt);
  const last = lastAt === undefined || lastAt === null ? null : Date.parse(lastAt);
  const stale = last !== null && now - last > PLAN_READING_MAX_AGE;
  const reached = !stale && usedPercent >= 100 && reset !== null && reset > now;
  const usable = !stale && reset !== null && reset > now ? forecast ?? null : null;
  const warning = reached || usable!==null && usable.runs_out_at!==null;
  const text = stale
    ? t('planUsage.stopped',{time:formatSpan(now-last,format)})
    : reached ? t('planUsage.reached',{time:formatSpan(Math.max(0,reset-now),format)})
      : usable === null ? t('planUsage.insufficient')
        : usable.runs_out_at !== null ? t('planUsage.runsOut',{basis:t(`planUsage.basis.${usable.basis}`),time:forecastTime(Date.parse(usable.runs_out_at),now,format)})
          : t(cap ? 'planUsage.byEnd' : 'planUsage.atReset',{basis:t(`planUsage.basis.${usable.basis}`),percent:formatNumber(usable.percent_at_end,format.locale,{maximumFractionDigits:0}),time:reset === null ? '' : forecastTime(reset,now,format)});
  return <p data-forecast-state={stale ? 'stale' : reached ? 'reached' : usable === null ? 'insufficient' : usable.runs_out_at !== null ? 'runs_out' : 'at_reset'} className={cx('text-sm tabular-nums',warning ? 'text-warning' : 'text-text-muted')}>{text}</p>;
}
