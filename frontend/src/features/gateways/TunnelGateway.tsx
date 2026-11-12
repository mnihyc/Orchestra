/** The actual tool carrier is separate from each connector's primary gateway. */
import { useTranslation } from 'react-i18next';
import type { GatewayCatalogueEntry, ThreadTunnelGatewayStatus } from '@/generated/api-types';
import { InlineAlert } from '@/ui/InlineAlert';
import { GatewayName } from './GatewayName';
import { placementDecision } from './placement';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatSpan } from '@/lib/time';
import { RelativeTime } from '@/ui/RelativeTime';

export function TunnelGateway({ status, gateways }: {
  status: ThreadTunnelGatewayStatus; gateways: readonly GatewayCatalogueEntry[];
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const decision = placementDecision(status.plan);
  const fallback = status.state === 'attached' && status.attached_gateway_id !== null && status.plan !== null && status.attached_gateway_id !== status.plan.preferred_gateway.id;
  const available = [...gateways, ...(status.plan === null ? [] : [status.plan.preferred_gateway, ...status.plan.common_fallbacks])];
  if (status.state === 'unavailable' && status.refusal !== null) return <InlineAlert tone="warning" className="min-w-0 flex-1 wrap-anywhere" title={t('regionalGateways.unreachable')}>
    <span>{status.refusal.detail}</span>
    {status.refusal.attempted_gateways.length > 0 && <ul aria-label={t('regionalGateways.attempted')} className="mt-1 flex min-w-0 flex-wrap gap-x-3 gap-y-1 text-xs">
      {[...new Set(status.refusal.attempted_gateways)].map((id) => <li key={id}><GatewayName id={id} gateways={available} /></li>)}
    </ul>}
  </InlineAlert>;
  return <span className="inline-flex min-w-0 max-w-full flex-wrap items-baseline gap-x-2 gap-y-1 text-xs leading-(--control-sm) text-text-muted" aria-label={t('regionalGateways.toolConnection')}>
    <span>{t('regionalGateways.toolConnection')}:</span>
    {status.state === 'attached' && status.attached_gateway_id !== null
      ? <GatewayName id={status.attached_gateway_id} gateways={available} />
      : <span>{t(`regionalGateways.tunnelState.${status.state}`)}</span>}
    {fallback && <span>{t('regionalGateways.fallback')}</span>}
    {decision !== null && <span aria-label={t('regionalGateways.placement')} className="inline-flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1 tabular-nums">
      {fallback && status.plan !== null && <span className="inline-flex min-w-0 flex-wrap gap-x-2"><span>{t('regionalGateways.plannedGateway')}:</span><GatewayName id={status.plan.preferred_gateway.id} gateways={available} /></span>}
      <span>{t(`regionalGateways.placementRule.${decision.rule}`)}</span>
      {decision.combined_round_trip_ms !== null && <span>{t('regionalGateways.combinedRoundTrip', { value: formatNumber(decision.combined_round_trip_ms, locale) })}</span>}
      <span>{t('regionalGateways.sampleLimit', { duration: formatSpan(decision.max_sample_age_secs * 1000, { locale }) })}</span>
      {decision.source_measured_at === decision.destination_measured_at
        ? <span>{t(decision.source_measured_at === null ? 'regionalGateways.measurements' : 'regionalGateways.bothMeasured')} {decision.source_measured_at === null ? t('regionalGateways.notMeasured') : <RelativeTime at={decision.source_measured_at} />}</span>
        : <><span>{t('regionalGateways.sourceMeasured')} {decision.source_measured_at === null ? t('regionalGateways.notMeasured') : <RelativeTime at={decision.source_measured_at} />}</span>
          <span>{t('regionalGateways.executorMeasured')} {decision.destination_measured_at === null ? t('regionalGateways.notMeasured') : <RelativeTime at={decision.destination_measured_at} />}</span></>}

    </span>}
  </span>;
}
