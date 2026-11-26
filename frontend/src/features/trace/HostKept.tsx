/** A host manager sees kept disk and unresolved cleanup without receiving an early-purge act. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { hostChildrenQuery } from '@/api/resources';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { ScrollRegion } from '@/ui/ScrollRegion';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { HardDrive } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { Host } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Badge } from '@/ui/Badge';
import { EmptyState } from '@/ui/EmptyState';
import { retainedJobWords } from './jobWords';

export function HostKept({ host }: { host: Pick<Host, 'id' | 'kept_data' | 'access'> }) {
  const search = useCollectionSearch('host-kept:' + host.id);
  if (!holds(host.access, 'host.manage')) return null;
  return <HostKeptRows key={search.scope} host={host} search={search} />;
}
function HostKeptRows({ host, search }: { host: Pick<Host, 'id' | 'kept_data' | 'access'>; search: ReturnType<typeof useCollectionSearch> }) {
  const { t } = useTranslation(); const format = useFormatContext();
  const source = host.kept_data;
  const version = useMemo(() => JSON.stringify(source?.map((row) => [row.environment_id, row.name, row.home?.name, row.storages, row.phase, row.purge_at, row.job?.id, row.job?.state])), [source]);
  const paged = (source?.length ?? 0) > 50 || search.q !== '';
  const pages = useInfiniteQuery({ ...hostChildrenQuery(host.id, 'kept_data', search.q, version), enabled: paged });
  const rows = paged ? pages.data?.pages.flatMap((page) => page.items) ?? [] : source ?? [];
  const all = source ?? [];
  const bytes = all.reduce((sum, row) => sum + (row.bytes ?? 0), 0);
  const unknown = all.filter((row) => row.bytes === null).length;
  const day = (value: string | null) => value === null ? t('trace.unknown') : new Intl.DateTimeFormat(format.locale, { dateStyle: 'medium', timeZone: format.timeZone }).format(new Date(value));
  return <Section id="host-kept" title={t('trace.keptTitle')} lead={t('trace.keptLead')}>
    {all.length>0 && <p className="flex flex-wrap gap-x-3 gap-y-1 text-sm text-text-muted"><span>{t('trace.hostMeasured',{count:all.length,size:formatBytes(bytes,format.locale)})}</span>{unknown>0 && <span>{t('trace.hostUnmeasured',{count:unknown})}</span>}</p>}
    <CollectionSearch name={t('trace.keptTitle')} {...search} />
    {paged && pages.isPending ? <SkeletonRows read={pages} rows={3} /> : paged && pages.isError && !pages.isFetchNextPageError ? <ErrorState compact error={pages.error} onRetry={() => void pages.refetch()} /> : rows.length === 0 ? <EmptyState compact icon={<HardDrive aria-hidden />} title={t('trace.keptEmpty')} /> : <ScrollRegion kind="table" label={t('trace.keptTitle')} className="max-h-96 overflow-y-auto"><ul className="divide-y divide-border">{rows.map((entry) => <li key={entry.environment_id} data-kept-environment={entry.environment_id} className="flex flex-col gap-2 py-4">
      <div className="flex flex-wrap items-center gap-2"><span className="font-medium wrap-anywhere">{entry.name ?? t('placeReview.privateEnvironment')}</span><Badge tone="warning">{t(`trace.phases.${entry.phase}`)}</Badge></div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-sm">
        <dt className="text-text-muted">{t('trace.size')}</dt><dd>{entry.bytes === null ? t('trace.unknown') : formatBytes(entry.bytes, format.locale)}</dd>
        <dt className="text-text-muted">{t('trace.home')}</dt><dd className="wrap-anywhere">{entry.home?.redacted === true ? t('trace.unknown') : entry.home?.name ?? t('trace.unknown')}</dd>
        <dt className="text-text-muted">{t('trace.storage')}</dt><dd className="wrap-anywhere">{entry.storages.map((storage) => storage ?? t('hosts.storages.default')).join(', ') || t('trace.unknown')}</dd>
      </dl>
      <p className="text-sm text-text-muted">{t('trace.until', { day: day(entry.purge_at) })}</p>
      {entry.job !== null && <p className="text-sm wrap-anywhere text-text-muted">{retainedJobWords(t,entry.job)}</p>}
    </li>)}</ul></ScrollRegion>}
    <Pagination hasMore={paged && pages.hasNextPage} loading={pages.isFetchingNextPage} error={paged && pages.isFetchNextPageError ? pages.error : null} onMore={() => void pages.fetchNextPage()} />
  </Section>;
}
