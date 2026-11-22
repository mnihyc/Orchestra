/** A host shows each environment's kept installed-file layers, including archived environments with no unit. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { useMemo } from 'react';
import { hostChildrenQuery } from '@/api/resources';
import { useCollectionSearch } from '@/lib/collectionSearch';
import { CollectionSearch } from '@/ui/CollectionSearch';
import { Pagination } from '@/ui/Pagination';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { EmptyState } from '@/ui/EmptyState';
import { Link } from '@tanstack/react-router';
import { useTranslation } from 'react-i18next';
import type { Host, HostKeptLayers } from '@/generated/api-types';
import { formatBytes } from '@/lib/format';
import { holds } from '@/lib/permissions';
import { Section } from '@/shell/Page';
import { Table, type Column } from '@/ui/Table';
import { StateLine } from '@/ui/StateLine';

export function KeptLayers({ host }: { host: Host }) {
  const { t, i18n } = useTranslation();
  const source = host.kept_layers;
  const search = useCollectionSearch('host-layers:' + host.id);
  const version = useMemo(() => JSON.stringify(source?.map((row) => [row.environment_id, row.generation, row.environment?.name, row.bytes, row.latest])), [source]);
  const paged = (source?.length ?? 0) > 50 || search.q !== '';
  const pages = useInfiniteQuery({ ...hostChildrenQuery(host.id, 'kept_layers', search.q, version), enabled: paged });
  const rows = paged ? pages.data?.pages.flatMap((page) => page.items) ?? [] : source ?? [];
  if ((source?.length ?? 0) === 0 && search.text === '') return <Section id="host-kept-layers" title={t('keptLayers.title')}>
    <StateLine compact message={t(host.kept_layers === undefined || !(holds(host.access, 'host.view') || holds(host.access, 'host.manage')) ? 'implicitAvailability.detailsUnavailable' : 'implicitAvailability.noKeptLayers')} />
  </Section>;
  const columns: Column<HostKeptLayers>[] = [
    { id: 'environment', header: t('keptLayers.environment'), cell: (row) => row.environment === null ? t('ui.nameUnknown') : <Link to="/resources/environments/$id" params={{ id: row.environment_id }} className="text-link hover:underline">{row.environment.name}</Link> },
    { id: 'which', header: t('keptLayers.which'), cell: (row) => t(row.latest ? 'keptLayers.latest' : 'keptLayers.earlier') },
    { id: 'bytes', header: t('keptLayers.size'), cell: (row) => formatBytes(row.bytes, i18n.resolvedLanguage ?? 'en') },
  ];
  return <Section id="host-kept-layers" title={t('keptLayers.title')} lead={t('keptLayers.lead')}><CollectionSearch name={t('keptLayers.title')} {...search} />
    {paged && pages.isPending ? <SkeletonRows read={pages} rows={3} /> : paged && pages.isError && !pages.isFetchNextPageError ? <ErrorState compact error={pages.error} onRetry={() => void pages.refetch()} /> : rows.length === 0 ? <EmptyState compact title={t('ui.collection.none')} /> : <Table key={search.q} virtual label={t('keptLayers.title')} columns={columns} rows={rows} rowKey={(row) => `${row.environment_id}:${row.generation}`} />}
    <Pagination hasMore={paged && pages.hasNextPage} loading={pages.isFetchingNextPage} error={paged && pages.isFetchNextPageError ? pages.error : null} onMore={() => void pages.fetchNextPage()} /></Section>;
}
