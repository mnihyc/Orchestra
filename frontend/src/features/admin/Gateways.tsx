/** System administrators edit the ordered advertisements; a successful receipt is read back. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { gatewayCatalogueQuery, updateGatewayCatalogue } from '@/api/gateways';
import type { GatewayCatalogueUpdateBody } from '@/generated/api-types';
import { GatewayCatalogueView } from '@/features/gateways/GatewayCatalogue';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';

export function Gateways() {
  const { t } = useTranslation();
  const query = useQuery(gatewayCatalogueQuery);
  const save = async (body: GatewayCatalogueUpdateBody) => {
    await updateGatewayCatalogue(body);
  };
  if (query.isPending) return <SkeletonRows rows={3} />;
  if (query.data === undefined) return <ErrorState compact title={t('regionalGateways.loadFailed')} error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />;
  return <div className="flex min-w-0 flex-col gap-3">
    {query.isError && <ErrorState compact title={t('regionalGateways.loadFailed')} error={query.error} retrying={query.isRefetching} onRetry={() => void query.refetch()} />}
    <GatewayCatalogueView catalogue={query.data} writable onSave={save} refreshing={query.isFetching} onRefresh={() => void query.refetch()} />
  </div>;
}
