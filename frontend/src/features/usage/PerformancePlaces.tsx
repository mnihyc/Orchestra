/** Seven-day performance where people choose models, inspect a provider, or see its current degradation. */
import { useQuery } from '@tanstack/react-query';
import { Link, useNavigate } from '@tanstack/react-router';
import { Gauge } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { healthQuery, usageQuery, type UsageParams } from '@/api/usage';
import type { Thread, UsageComparison } from '@/generated/api-types';
import { PerformanceFigures } from '@/features/usage/Performance';
import { choiceObservation, errorRate, rateText, relativeSpeed, speedText, timeText } from '@/features/usage/performance';
import { spanOf } from '@/features/usage/search';
import { useFormatContext } from '@/lib/appearance';
import { useNow } from '@/lib/time';
import { boxClasses } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { Sparkline } from '@/ui/Sparkline';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusPart } from '@/ui/StatusBar';

/** The same UTC seven-day span and scope as Usage; the server remains the authority. */
export function recentPerformance(scope: string, now: number, health = false): UsageParams {
  const span = spanOf({ range: '7d' }, now);
  return { scope, from: span.from, to: span.to, bucket: 'day', measure: 'speed', breakdowns: [], filters: {}, ...(health ? { health: true } : {}) };
}

export function useChoicePerformance(thread: Pick<Thread,'topic_id'> & {binding:{provider:{id:string}}|null}) {
  const [now] = useState(Date.now);
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const provider = thread.binding?.provider;
  const params = recentPerformance(`node:${thread.topic_id}`, now);
  params.filters = provider === undefined ? {} : { provider_id: provider.id };
  const query = useQuery({ ...usageQuery(params), enabled: provider !== undefined, retry: false });
  const rows = query.isError || query.isPlaceholderData ? [] : query.data?.comparison ?? [];
  return (model: string, effort: string | null, tier: string | null, program: string | null, gain = false): string | null => {
    const row = choiceObservation(rows, model, effort, tier, program);
    if (row === null) return null;
    const p = row.performance;
    const hint = t('usagePerf.choiceHint', { speed: speedText((p.speed_milli ?? 0) / 1000, locale, t), first: timeText(p.first_token_ms, locale, t) });
    const ratio = gain ? measuredGain(row, rows) : null;
    return ratio === null ? hint : `${hint}. ${t(ratio > 1 ? 'usagePerf.faster' : 'usagePerf.relativeSpeed', { ratio: new Intl.NumberFormat(locale, { maximumFractionDigits: 1 }).format(ratio) })}`;
  };
}

function measuredGain(row: UsageComparison, rows: readonly UsageComparison[]) {
  const measured = rows.filter((other) => choiceObservation([other], other.model, other.effort, other.tier, other.program) !== null);
  return relativeSpeed(row, measured);
}

export function ProviderPerformance({ providerId }: { providerId: string }) {
  const { t } = useTranslation();
  const [now] = useState(Date.now);
  const query = useQuery(usageQuery(recentPerformance(`provider:${providerId}`, now)));
  const daily = query.data?.series.map((point) => { const speed = point.figures.performance?.speed_milli; return speed === undefined || speed === null ? null : speed / 1000; }) ?? [];
  return <section aria-label={t('usagePerf.providerPerformance')} className={`${boxClasses} flex min-w-0 flex-col gap-3 p-4`}>
    <h2 className="text-base font-semibold">{t('usagePerf.providerPerformance')}</h2>
    {query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.isPending ? <SkeletonRows rows={3} /> : <><PerformanceFigures current={query.data.totals.performance} /><Sparkline className="h-12!" label={t('usagePerf.dailySpeed')} series={[{ values: daily }]} /></>}
    <Link to="/usage" search={{ scope: `provider:${providerId}`, view: 'performance', range: '7d' }} className="self-start text-sm text-accent-text hover:underline">{t('usagePerf.openPerformance')}</Link>
  </section>;
}

/** An open ring or Usage panel owns health loading and its scoped live subscription. */
export function useProviderHealth(providerId: string | null) {
  return useQuery({ ...healthQuery(providerId ?? ''), enabled: providerId !== null, retry: false });
}

/** Thresholds and scope are evaluated by the API; status only observes health a visible panel has loaded. */
export function ProviderDegradedPart({ thread }: { thread: {binding:Pick<NonNullable<Thread['binding']>,'provider'>|null} }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const now = useNow();
  const navigate = useNavigate();
  const provider = thread.binding?.provider;
  const query = useQuery({ ...healthQuery(provider?.id ?? ''), enabled: false, retry: false });
  const health = query.data;
  if (provider === undefined || query.isError || query.isPlaceholderData || health === undefined || (!health.errors && !health.first_output)) return null;
  const label = health.errors ? t('usagePerf.degradedErrors', { provider: provider.name, rate: rateText(errorRate(health.recent), locale, t) }) : t('usagePerf.degradedFirst', { provider: provider.name, time: timeText(health.recent.first_token_ms, locale, t) });
  return <StatusPart tone="warning" icon={<Gauge size={16} aria-hidden />} hint={t('usagePerf.degradedHint')} onOpen={() => void navigate({ to: '/usage', search: { scope: `provider:${provider.id}`, view: 'requests', from: health.since.slice(0, 10), to: new Date(now).toISOString().slice(0, 10), outcome: 'errors' } })}>{label}</StatusPart>;
}
