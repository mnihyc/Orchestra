/** This thread's single ledger, permitted provider observations and applicable caps view. */
import { useQuery } from '@tanstack/react-query';
import { ChartNoAxesColumn, RefreshCw } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { ApiError } from '@/api/client';
import { threadLedgerQuery } from '@/api/provider-plan';
import { providerQuery } from '@/api/resources';
import { useRouteThread } from '@/features/files/door';
import { LedgerFigures } from '@/features/resources/providers/Ledger';
import { Limits } from '@/features/usage/Limits';
import { ThreadComposition } from '@/features/usage/context/ThreadComposition';
import { meteringOf } from '@/features/usage/ThreadUsage';
import { useProviderHealth } from '@/features/usage/PerformancePlaces';
import { PlanPanel } from '@/features/resources/providers/PlanPanel';
import { privateProvider } from '@/features/resources/providers/plan';
import type { ViewProps } from '@/features/workbench/views';
import type { Thread } from '@/generated/api-types';
import { useThreadView } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { ScrollArea } from '@/ui/ScrollArea';
import { useNow } from '@/lib/time';

export function ThreadUsage({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const now = useNow();
  const ledger = useQuery(threadLedgerQuery(thread.id, thread.created_at, now));
  const providerId = thread.binding?.provider.id ?? '';
  useProviderHealth(thread.binding?.provider.id ?? null);
  const provider = useQuery({ ...providerQuery(providerId), enabled: providerId !== '', retry: false });
  const denied = provider.error instanceof ApiError && (provider.error.status === 403 || provider.error.status === 404);
  const readable = !denied && provider.data !== undefined && !privateProvider(provider.data);
  return <div className="flex min-w-0 flex-col gap-4 p-4" data-thread-usage>
    <section className="flex flex-col gap-4 rounded-lg border border-border bg-surface p-4" aria-label={t('providerPlan.thisThread')}>
      <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-base font-semibold text-text">{t('providerPlan.thisThread')}</h2><Button size="sm" icon={<RefreshCw aria-hidden />} pending={ledger.isFetching} onClick={() => void ledger.refetch()}>{t('providers.overview.refresh')}</Button></div>
      <p className="text-sm text-text-muted">{t('providerPlan.ledgerLead')}</p>
      {ledger.isError ? <ErrorState compact error={ledger.error} onRetry={() => void ledger.refetch()} /> : ledger.data === undefined ? <SkeletonRows rows={3} /> : <><LedgerFigures figures={ledger.data.totals} /><p className="text-xs text-text-subtle">{t('providerPlan.ledgerSource')} <RelativeTime at={new Date(ledger.dataUpdatedAt).toISOString()} /></p></>}
    </section>
    {/* What its turns used over time: the parts of their tokens, the context's fill and its shortenings. */}
    <ThreadComposition thread={thread} />
    {thread.binding !== null && <section className="flex flex-col gap-2" aria-label={t('providerPlan.providerTitle', { name: thread.binding.provider.name })}>
      <h2 className="text-base font-semibold text-text">{t('providerPlan.providerTitle', { name: thread.binding.provider.name })}</h2>
      {readable ? <PlanPanel provider={provider.data} /> : provider.isPending ? <SkeletonRows rows={2} /> : <div className="rounded-lg border border-border bg-surface p-4 text-sm text-text-muted"><p>{t('providerPlan.viewMissing')}</p><p>{thread.blocked?.detail ?? t(thread.binding.provider.status === 'exhausted' ? 'providerPlan.recovery.limit' : thread.binding.provider.status === 'error' || thread.binding.provider.status === 'needs_setup' ? 'providerPlan.recovery.signin' : 'providerPlan.recovery.unavailable')}</p></div>}
    </section>}
    <Limits of={{ kind:'node',id:thread.id,name:thread.name,manage:thread.access.permissions.includes('usage.manage') }} metering={meteringOf(thread)} />
  </div>;
}

export function ThreadUsageView(_props: ViewProps) {
  const { t } = useTranslation();
  const threadId = useRouteThread();
  const view = useThreadView(threadId ?? '');
  if (view.closed !== null) return <ErrorState compact error={new Error(view.closed)} />;
  if (view.thread === null) return <SkeletonRows rows={3} />;
  return <ScrollArea className="focus-inset h-full" role="region" tabIndex={0} aria-label={t('providerPlan.threadTitle')}><ThreadUsage thread={view.thread} /></ScrollArea>;
}

export const USAGE_ICON = ChartNoAxesColumn;

