/** Full ordered catalogue editing preserves the revision that was actually reviewed. */
import { useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { GatewayCatalogue, GatewayCatalogueEntry, GatewayCatalogueUpdateBody, GatewayEndpointTrust } from '@/generated/api-types';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert } from '@/ui/ErrorState';
import { Input } from '@/ui/Input';
import { Select } from '@/ui/Select';
import { HelpHint } from '@/ui/HelpHint';
import { IconButton } from '@/ui/IconButton';

/** The regional design admits at most five logical gateways, regardless of endpoint aliases. */
const GATEWAYS_MAX = 5;
interface Draft { key: number; existing: boolean; gateway: GatewayCatalogueEntry }

export function GatewayCatalogueView({ catalogue, writable = false, onSave, onRefresh, refreshing = false }: {
  catalogue: GatewayCatalogue; writable?: boolean;
  onSave: (body: GatewayCatalogueUpdateBody) => Promise<void>;
  onRefresh?: () => void; refreshing?: boolean;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<Draft[]>([]);
  const [revision, setRevision] = useState(catalogue.revision);
  const nextKey = useRef(0);
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const change = (key: number, update: (gateway: GatewayCatalogueEntry) => GatewayCatalogueEntry) => {
    setDraft((current) => current.map((row) => row.key === key ? { ...row, gateway: update(row.gateway) } : row));
  };
  const begin = () => {
    setDraft(catalogue.gateways.map((gateway) => ({ key: nextKey.current++, existing: true,
      gateway: { ...gateway, endpoints: gateway.endpoints.map((endpoint) => ({ ...endpoint })) } })));
    setRevision(catalogue.revision); setError(null); setEditing(true);
  };
  const add = () => {
    const key = nextKey.current++;
    setDraft((current) => [...current, { key, existing: false,
      gateway: { id: '', label: '', region: '', endpoints: [{ url: '', trust: 'pinned' }] } }]);
  };
  const move = (key: number, direction: -1 | 1) => {
    setDraft((current) => {
      const from = current.findIndex((row) => row.key === key);
      const to = from + direction;
      if (from < 0 || to < 0 || to >= current.length) return current;
      const next = [...current];
      const [row] = next.splice(from, 1);
      if (row !== undefined) next.splice(to, 0, row);
      return next;
    });
  };
  const save = async () => {
    setPending(true); setError(null);
    try {
      await onSave({ expected_revision: revision, gateways: draft.map((row) => row.gateway) });
      setEditing(false);
    } catch (refusal) { setError(refusal); }
    finally { setPending(false); }
  };
  return <section aria-label={t('regionalGateways.title')} className="flex min-w-0 flex-col gap-3">
    <div className="flex min-w-0 flex-wrap items-baseline justify-between gap-2">
      <span className="min-w-0 wrap-anywhere text-xs text-text-muted">{t('regionalGateways.revision', { revision: catalogue.revision })}</span>
      <span className="inline-flex items-center gap-1"><HelpHint term={t('regionalGateways.title')} what={t('regionalGateways.catalogueHelp')}
        guide={{ article: 'connect-a-machine', heading: 'gateway-catalogue' }} />
        {onRefresh !== undefined && <IconButton action="refresh" size="sm" label={t('regionalGateways.refresh')} pending={refreshing} onClick={onRefresh} />}
        <Button size="sm" variant="secondary" disabled={!writable} onClick={begin}>{t('regionalGateways.edit')}</Button></span>
    </div>
    {catalogue.gateways.length === 0 ? <EmptyState compact title={t('regionalGateways.empty')} />
      : <ul aria-label={t('regionalGateways.catalogue')} className="divide-y divide-border rounded-lg border border-border px-3">
        {catalogue.gateways.map((gateway) => <li key={gateway.id} className="grid min-w-0 grid-cols-1 gap-x-4 gap-y-2 py-3 sm:grid-cols-[minmax(8rem,1fr)_minmax(0,2fr)]">
          <div className="flex min-w-0 flex-col gap-1">
            <span className="font-medium wrap-anywhere">{gateway.label}</span>
            <span className="text-sm wrap-anywhere text-text-muted">{gateway.region}</span>
            <span className="text-xs wrap-anywhere text-text-subtle">{gateway.id}</span>
          </div>
          <ul aria-label={t('regionalGateways.endpointsFor', { label: gateway.label })} className="flex min-w-0 flex-col gap-1 text-sm">
            {gateway.endpoints.map((endpoint) => <li key={`${endpoint.trust}:${endpoint.url}`} className="flex min-w-0 flex-wrap items-baseline gap-x-2 gap-y-1">
              <code className="min-w-0 wrap-anywhere font-mono text-xs">{endpoint.url}</code>
              <span className="text-xs text-text-muted">{t(`regionalGateways.trust.${endpoint.trust}`)}</span>
            </li>)}
          </ul>
        </li>)}
      </ul>}
    <Dialog open={editing} onOpenChange={(open) => { if (!pending) setEditing(open); }} title={t('regionalGateways.editTitle')} size="lg" dismissible={!pending}
      footer={<><Button disabled={pending} onClick={() => setEditing(false)}>{t('common.cancel')}</Button>
        <Button variant="primary" pending={pending} disabled={!writable} onClick={() => void save()}>{t('common.save')}</Button></>}>
      <div className="flex min-w-0 flex-col gap-3 py-2">
        {error !== null && <ErrorAlert error={error} />}
        {draft.map(({ key, existing, gateway }, index) => <fieldset key={key} className="flex min-w-0 flex-col gap-3 rounded-lg border border-border p-3">
          <legend className="px-1 text-sm font-medium">{t('regionalGateways.gatewayNumber', { number: index + 1 })}</legend>
          <div className="flex flex-wrap gap-2">
            <Button size="sm" variant="ghost" disabled={pending || index === 0} aria-label={t('regionalGateways.moveUp', { label: gateway.label })} onClick={() => move(key, -1)}>{t('regionalGateways.up')}</Button>
            <Button size="sm" variant="ghost" disabled={pending || index === draft.length - 1} aria-label={t('regionalGateways.moveDown', { label: gateway.label })} onClick={() => move(key, 1)}>{t('regionalGateways.down')}</Button>
          </div>
          <div className="grid min-w-0 grid-cols-1 gap-3 sm:grid-cols-2">
            <Input label={t('regionalGateways.id')} value={gateway.id} readOnly={existing} disabled={pending} autoComplete="off"
              onChange={(event) => change(key, (value) => ({ ...value, id: event.target.value }))} />
            <Input label={t('regionalGateways.label')} value={gateway.label} disabled={pending}
              onChange={(event) => change(key, (value) => ({ ...value, label: event.target.value }))} />
            <Input label={t('regionalGateways.region')} value={gateway.region} disabled={pending}
              onChange={(event) => change(key, (value) => ({ ...value, region: event.target.value }))} />
          </div>
          {gateway.endpoints.map((endpoint, at) => <div key={at} className="flex min-w-0 flex-wrap items-end gap-2">
            <Input label={t('regionalGateways.endpointNumber', { number: at + 1 })} value={endpoint.url} disabled={pending} fieldClassName="min-w-0 flex-[1_1_14rem]"
              onChange={(event) => change(key, (value) => ({ ...value, endpoints: value.endpoints.map((old, n) => n === at ? { ...old, url: event.target.value } : old) }))} />
            <Select<GatewayEndpointTrust> label={t('regionalGateways.trustLabel')} value={endpoint.trust} disabled={pending} fieldClassName="min-w-[7rem] flex-[0_1_8rem]"
              options={(['pinned', 'public'] as const).map((trust) => ({ value: trust, name: t(`regionalGateways.trust.${trust}`) }))}
              onValueChange={(trust) => change(key, (value) => ({ ...value, endpoints: value.endpoints.map((old, n) => n === at ? { ...old, trust } : old) }))} />
            <Button size="sm" variant="ghost" disabled={pending} aria-label={t('regionalGateways.removeEndpointNumber', { number: at + 1, label: gateway.label })}
              onClick={() => change(key, (value) => ({ ...value, endpoints: value.endpoints.filter((_, n) => n !== at) }))}>{t('regionalGateways.remove')}</Button>
          </div>)}
          <div className="flex flex-wrap items-center justify-between gap-2">
            <Button size="sm" variant="ghost" disabled={pending} onClick={() => change(key, (value) => ({ ...value, endpoints: [...value.endpoints, { url: '', trust: 'pinned' }] }))}>{t('regionalGateways.addEndpoint')}</Button>
            <Button size="sm" variant="ghost" disabled={pending} aria-label={t('regionalGateways.removeGateway', { label: gateway.label })}
              onClick={() => setDraft((current) => current.filter((row) => row.key !== key))}>{t('regionalGateways.removeGateway', { label: gateway.label })}</Button>
          </div>
        </fieldset>)}
        <Button size="sm" variant="secondary" disabled={pending || draft.length >= GATEWAYS_MAX} onClick={add}>{t('regionalGateways.addGateway')}</Button>
      </div>
    </Dialog>
  </section>;
}
