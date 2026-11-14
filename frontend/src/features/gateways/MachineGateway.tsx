/** The connector reports its primary and persisted choice; the web does not rewrite either. */
import { useTranslation } from 'react-i18next';
import { type ReactNode, useMemo, useState } from 'react';
import type { ConnectorGatewayStatus, GatewayCatalogueEntry } from '@/generated/api-types';
import { Facts } from '@/features/resources/Facts';
import { useAppearance } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { RelativeTime } from '@/ui/RelativeTime';
import { GatewayName } from './GatewayName';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { Button } from '@/ui/Button';
import { RESOURCE_PAGE_SIZE } from '@/api/resources';

export function MachineGatewayCard({ status, gateways, onRefresh, refreshing = false, actions, children }: {
  status: ConnectorGatewayStatus; gateways: readonly GatewayCatalogueEntry[];
  onRefresh?: () => void; refreshing?: boolean;
  actions?:ReactNode;children?:ReactNode;
}) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const [search,setSearch]=useState(''); const [offset,setOffset]=useState(0);
  const needle=search.trim().toLocaleLowerCase();
  const found=needle===''?gateways:gateways.filter((gateway)=>[gateway.label,gateway.region,gateway.id].some((value)=>value.toLocaleLowerCase().includes(needle)));
  const start=offset<found.length?offset:0;
  const measuredByGateway=useMemo(()=>{const result=new Map<string,number>();for(const sample of status.measurements)result.set(sample.gateway_id,Math.min(result.get(sample.gateway_id)??sample.round_trip_ms,sample.round_trip_ms));return result;},[status.measurements]);
  return <section aria-label={t('regionalGateways.connector')} className="flex min-w-0 flex-col gap-2 rounded-lg border border-border p-3">
    <div className="flex min-w-0 items-center justify-between gap-2">
      <div className="inline-flex min-w-0 items-center gap-1"><h3 className="text-sm font-medium">{t('regionalGateways.connector')}</h3>
        <HelpHint term={t('regionalGateways.primary')} what={t('regionalGateways.primaryHelp')}
          guide={{ article: 'connect-a-machine', heading: 'choose-a-gateway' }} /></div>
      <div className="flex items-center gap-1">{actions}{onRefresh !== undefined && <IconButton action="refresh" size="sm" label={t('regionalGateways.refreshConnector')} pending={refreshing} onClick={onRefresh} />}</div>
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
      {gateways.length>10&&<Input type="search" label={t('regionalGateways.findGateway')} hideLabel value={search} onChange={(event)=>{setSearch(event.target.value);setOffset(0);}}/>}
      <ul aria-label={t('regionalGateways.measurements')} className="max-h-80 overflow-y-auto divide-y divide-border text-sm">
        {found.slice(start,start+RESOURCE_PAGE_SIZE).map((gateway) => {
          const measured=measuredByGateway.get(gateway.id)??null;
          return <li key={gateway.id} className="flex min-h-(--control-sm) min-w-0 flex-wrap items-baseline justify-between gap-x-3 gap-y-1 py-1">
            <GatewayName id={gateway.id} gateways={[gateway]} />
            <span className="shrink-0 tabular-nums text-text-muted">{measured === null ? t('regionalGateways.notMeasured') : t('regionalGateways.milliseconds', { value: formatNumber(measured, locale) })}</span>
          </li>;
        })}
      </ul>
      {found.length>RESOURCE_PAGE_SIZE&&<div className="flex justify-between gap-2"><Button size="sm" disabled={start===0} onClick={()=>setOffset(0)}>{t('documentFolders.beginning')}</Button><Button size="sm" disabled={start+RESOURCE_PAGE_SIZE>=found.length} onClick={()=>setOffset(start+RESOURCE_PAGE_SIZE)}>{t('documentFolders.next')}</Button></div>}
    </>}
    {children}
  </section>;
}
