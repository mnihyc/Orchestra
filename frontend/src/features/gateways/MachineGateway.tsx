/** The connector reports its primary and persisted choice; the web does not rewrite either. */
import { useTranslation } from 'react-i18next';
import type { ConnectorGatewayStatus, GatewayCatalogueEntry } from '@/generated/api-types';
import { Facts } from '@/features/resources/Facts';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { RelativeTime } from '@/ui/RelativeTime';
import { GatewayName } from './GatewayName';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';

export function MachineGatewayCard({ status, gateways, onRefresh, refreshing = false }: {
  status: ConnectorGatewayStatus; gateways: readonly GatewayCatalogueEntry[];
  onRefresh?: () => void; refreshing?: boolean;
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  return <section aria-label={t('regionalGateways.connector')} className="flex min-w-0 flex-col gap-2 rounded-lg border border-border p-3">
    <div className="flex min-w-0 items-center justify-between gap-2">
      <div className="inline-flex min-w-0 items-center gap-1"><h3 className="text-sm font-medium">{t('regionalGateways.connector')}</h3>
        <HelpHint term={t('regionalGateways.primary')} what={t('regionalGateways.primaryHelp')}
          guide={{ article: 'connect-a-machine', heading: 'choose-a-gateway' }} /></div>
      {onRefresh !== undefined && <IconButton action="refresh" size="sm" label={t('regionalGateways.refreshConnector')} pending={refreshing} onClick={onRefresh} />}
    </div>
    <Facts compact label={t('regionalGateways.connector')} rows={[
      { name: t('regionalGateways.primary'), value: <GatewayName id={status.primary_gateway_id} gateways={gateways} /> },
      { name: t('regionalGateways.selection'), value: status.selection === null ? t('regionalGateways.notReported')
        : status.selection === 'automatic' ? t('regionalGateways.automatic') : <span className="inline-flex min-w-0 flex-wrap gap-x-2">
          <span>{t('regionalGateways.selected')}</span><GatewayName id={status.selected_gateway_id} gateways={gateways} missing="unavailable" />
        </span> },
      { name: t('regionalGateways.connection'), value: t(status.connected ? 'regionalGateways.connected' : 'regionalGateways.offline') },
    ]} />
    {status.selection === 'automatic' && <>
      <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2 text-xs text-text-muted">
        <span>{t('regionalGateways.measurements')}</span>
        {status.measured_at === null ? <span>{t('regionalGateways.notMeasured')}</span> : <span>{t('regionalGateways.measured')} <RelativeTime at={status.measured_at} /></span>}
      </div>
      <ul aria-label={t('regionalGateways.measurements')} className="divide-y divide-border text-sm">
        {gateways.map((gateway) => {
          const samples = status.measurements.filter((sample) => sample.gateway_id === gateway.id);
          const measured = samples.length === 0 ? null : Math.min(...samples.map((sample) => sample.round_trip_ms));
          return <li key={gateway.id} className="flex min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1 py-1">
            <GatewayName id={gateway.id} gateways={gateways} />
            <span className="shrink-0 tabular-nums text-text-muted">{measured === null ? t('regionalGateways.notMeasured') : t('regionalGateways.milliseconds', { value: formatNumber(measured, locale) })}</span>
          </li>;
        })}
      </ul>
    </>}
  </section>;
}
