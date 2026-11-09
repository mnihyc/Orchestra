/** Installation chooses a primary without adding roles or changing an issued token. */
import { useQuery } from '@tanstack/react-query';
import { useTranslation } from 'react-i18next';
import { gatewayCatalogueQuery } from '@/api/gateways';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { GatewayChoice } from './GatewayChoice';

export function InstallationGateway({ value, onChange, disabled = false }: { value: string; onChange: (value: string) => void; disabled?: boolean }) {
  const { t } = useTranslation();
  const catalogue = useQuery(gatewayCatalogueQuery);
  return <div className="flex min-w-0 flex-col gap-2">
    {catalogue.isPending && <SkeletonRows read={catalogue} rows={1} />}
    {catalogue.isError && <ErrorState compact title={t('regionalGateways.loadFailed')} error={catalogue.error} onRetry={() => void catalogue.refetch()} />}
    <GatewayChoice gateways={catalogue.data?.gateways ?? []} value={value} onChange={onChange} disabled={disabled || catalogue.isPending} />
  </div>;
}
