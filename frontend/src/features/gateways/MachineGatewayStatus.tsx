/** Hosts and personal-machine cards share the same connector status read. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { gatewayCatalogueQuery, machineGatewayQuery } from '@/api/gateways';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { MachineGatewayCard } from './MachineGateway';

export function MachineGatewayStatus({ machineId, online }: { machineId: string; online: boolean }) {
  const { t } = useTranslation();
  const status = useQuery(machineGatewayQuery(machineId, online));
  const catalogue = useQuery(gatewayCatalogueQuery);
  if (status.isPending || catalogue.isPending) return <SkeletonRows reads={[status, catalogue]} rows={2} />;
  if (status.isError || catalogue.isError) return <ErrorState compact title={t('regionalGateways.statusFailed')} error={status.error ?? catalogue.error}
    onRetry={() => { void status.refetch(); void catalogue.refetch(); }} />;
  return <MachineGatewayCard status={status.data} gateways={catalogue.data.gateways} refreshing={status.isFetching || catalogue.isFetching}
    onRefresh={() => { void status.refetch(); void catalogue.refetch(); }} />;
}
