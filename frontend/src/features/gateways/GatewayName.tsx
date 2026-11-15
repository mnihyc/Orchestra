/** Advertised labels join by stable identity; missing manual choices remain visible. */
import { useTranslation } from 'react-i18next';
import type { GatewayCatalogueEntry } from '@/generated/api-types';

export function GatewayName({ id, gateways, region = true, missing = 'identity' }: {
  id: string | null; gateways: readonly GatewayCatalogueEntry[]; region?: boolean; missing?: 'identity' | 'unavailable';
}) {
  const { t } = useTranslation();
  if (id === null) return <span className="text-text-muted">{t('regionalGateways.notReported')}</span>;
  const gateway = gateways.find((entry) => entry.id === id);
  if (gateway === undefined) return missing === 'unavailable'
    ? <span className="wrap-anywhere text-warning-text">{t('regionalGateways.unavailable')}</span>
    : <span className="wrap-anywhere text-text-muted">{id}</span>;
  return <span className="inline-flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 wrap-anywhere">
    <span>{gateway.label}</span>{region && gateway.region !== gateway.label && <span className="text-xs text-text-muted">{gateway.region}</span>}
  </span>;
}
