/** Bounded provider ledger summaries and applicable cap observations for provider and thread Usage. */
import { useQuery } from '@tanstack/react-query';
import { ArrowUpRight, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { planCapsQuery, providerLedgerQuery } from '@/api/provider-plan';
import type { UsageFigures } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber, formatTokens } from '@/lib/format';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Meter } from '@/ui/Meter';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { periodFigures } from './ledger';
import { capTitle } from '@/features/usage/words';

const PERIODS = ['today', 'sevenDays', 'month'] as const;

export function LedgerFigures({ figures }: { figures: UsageFigures }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const whollyUnrated = figures.tokens.total_tokens > 0 && figures.unrated_tokens === figures.tokens.total_tokens;
  return <div className="flex flex-col gap-2 text-sm">
    <p className="font-medium tabular-nums text-text">{t('providerPlan.tokens', { amount: formatTokens(figures.tokens.total_tokens, locale) })}</p>
    <p className="tabular-nums text-text-muted">{t('providerPlan.credits', { amount: whollyUnrated ? '–' : figures.credits })}</p>
    {Number(figures.estimated_credits) !== 0 && <p className="text-xs text-text-subtle">{t('providerPlan.estimated', { amount: figures.estimated_credits })}</p>}
    {figures.unrated_tokens > 0 && <p className="text-xs text-text-subtle">{t('providerPlan.unrated', { amount: formatNumber(figures.unrated_tokens, locale) })}</p>}
    {figures.turns === 0 && <p className="text-xs text-text-subtle">{t('providerPlan.noRecords')}</p>}
  </div>;
}

export function ProviderCounted({ providerId, onOpen }: { providerId: string; onOpen?: () => void }) {
  const { t } = useTranslation();
  const now = useNow();
  const query = useQuery(providerLedgerQuery(providerId, now));
  const figures = query.data === undefined ? null : periodFigures(query.data, now);
  return <section className="flex flex-col gap-4 rounded-lg border border-border bg-surface p-4" aria-label={t('providerPlan.ledger')}>
    <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-base font-semibold text-text">{t('providerPlan.ledger')}</h2><Button size="sm" icon={<RefreshCw aria-hidden />} pending={query.isFetching} onClick={() => void query.refetch()}>{t('providers.overview.refresh')}</Button></div>
    {query.isError ? <ErrorState compact error={query.error} onRetry={() => void query.refetch()} /> : figures === null ? <SkeletonRows rows={3} /> : <>
      <div className="grid gap-4 sm:grid-cols-3">{PERIODS.map((period) => <div key={period} className="flex flex-col gap-2"><h3 className="text-sm font-medium text-text">{t(`providerPlan.${period}`)}</h3><LedgerFigures figures={figures[period]} /></div>)}</div>
      <p className="text-xs text-text-subtle">{t('providerPlan.ledgerSource')} <RelativeTime at={new Date(query.dataUpdatedAt).toISOString()} /></p>
    </>}
    {onOpen !== undefined && <Button className="self-start" size="sm" icon={<ArrowUpRight aria-hidden />} onClick={onOpen}>{t('providerPlan.openUsage')}</Button>}
  </section>;
}

/** Uses the existing cap readers and Meter. The report owner's editor can replace this bounded summary. */
export function ApplicableLimits({ id, provider = false }: { id: string; provider?: boolean }) {
  const { t } = useTranslation();
  const query = useQuery(planCapsQuery(id, provider));
  return <section className="flex flex-col gap-4 rounded-lg border border-border bg-surface p-4" aria-label={t('providerPlan.limits')}>
    <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-base font-semibold text-text">{t('providerPlan.limits')}</h2><Button size="sm" icon={<RefreshCw aria-hidden />} pending={query.isFetching} onClick={() => void query.refetch()}>{t('providers.overview.refresh')}</Button></div>
    {query.isError ? <ErrorState compact error={query.error} /> : query.data === undefined ? <SkeletonRows rows={2} /> : query.data.length === 0 ? <EmptyState compact title={t('providerPlan.noLimits')} /> : <ul className="flex flex-col gap-4">{query.data.map((row) => <li key={`${row.cap.id}.${row.person?.id ?? ''}`} className="flex flex-col gap-2">
      <p className="text-sm text-text-muted">{t(`ui.limit.meter.${row.cap.meter}`)}</p>
      {row.cap.amount === null ? <p className="text-sm">{row.cap.note || capTitle(row.cap,t)}</p> : <Meter value={Number(row.used) / Number(row.cap.amount) * 100} label={row.cap.note || capTitle(row.cap,t)} detail={`${row.used} / ${row.cap.amount}`} />}
      {(row.state !== 'ok' || row.period_ends_at !== null) && <p className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted">{row.state !== 'ok' && <span>{row.state === 'reached' ? t('caps.row.reached') : t('caps.row.warning')}</span>}{row.period_ends_at !== null && <span>{t('providerDetails.resets')} <RelativeTime at={row.period_ends_at} /></span>}</p>}
    </li>)}</ul>}
    {query.data !== undefined && <p className="text-xs text-text-subtle">{t('providerPlan.ledgerSource')} <RelativeTime at={new Date(query.dataUpdatedAt).toISOString()} /></p>}
  </section>;
}

