/** Seven-day performance where people choose models or inspect a provider. */
import { useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { usageQuery, type UsageParams } from '@/api/usage';
import type { Thread, UsageComparison } from '@/generated/api-types';
import { PerformanceFigures } from '@/features/usage/Performance';
import { choiceObservation, relativeSpeed, speedText, timeText } from '@/features/usage/performance';
import { spanOf } from '@/features/usage/search';
import { useFormatContext } from '@/lib/appearance';
import { boxClasses } from '@/shell/Page';
import { ErrorState } from '@/ui/ErrorState';
import { Sparkline } from '@/ui/Sparkline';
import { SkeletonRows } from '@/ui/Skeleton';

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
    {query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : query.isPending ? <SkeletonRows read={query} rows={3} /> : <><PerformanceFigures current={query.data.totals.performance} /><Sparkline className="h-12!" label={t('usagePerf.dailySpeed')} series={[{ values: daily }]} /></>}
    <Link to="/usage" search={{ scope: `provider:${providerId}`, view: 'performance', range: '7d' }} className="self-start text-sm text-accent-text hover:underline">{t('usagePerf.openPerformance')}</Link>
  </section>;
}
