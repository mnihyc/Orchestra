/** The actual tool carrier is separate from each connector's primary gateway. */
import { useTranslation } from 'react-i18next';
import type { GatewayCatalogueEntry, ThreadTunnelGatewayStatus } from '@/generated/api-types';
import { InlineAlert } from '@/ui/InlineAlert';
import { GatewayName } from './GatewayName';

export function TunnelGateway({ status, gateways }: {
  status: ThreadTunnelGatewayStatus; gateways: readonly GatewayCatalogueEntry[];
}) {
  const { t } = useTranslation();
  const available = [...gateways, ...(status.plan === null ? [] : [status.plan.preferred_gateway, ...status.plan.common_fallbacks])];
  if (status.state === 'unavailable' && status.refusal !== null) return <InlineAlert tone="warning" className="min-w-0 flex-1 wrap-anywhere" title={t('regionalGateways.unreachable')}>
    <span>{status.refusal.detail}</span>
    {status.refusal.attempted_gateways.length > 0 && <ul aria-label={t('regionalGateways.attempted')} className="mt-1 flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs">
      {[...new Set(status.refusal.attempted_gateways)].map((id) => <li key={id}><GatewayName id={id} gateways={available} /></li>)}
    </ul>}
  </InlineAlert>;
  return <span className="inline-flex min-w-0 max-w-full flex-wrap items-baseline gap-x-2 gap-y-1 text-xs text-text-muted" aria-label={t('regionalGateways.tunnel')}>
    <span>{t('regionalGateways.tunnel')}:</span>
    {status.state === 'attached' && status.attached_gateway_id !== null
      ? <GatewayName id={status.attached_gateway_id} gateways={available} />
      : <span>{t(`regionalGateways.tunnelState.${status.state}`)}</span>}
  </span>;
}
