/** The host's three explicit port fields, with an unsaved proposal, affected-allocation review and acknowledgement. */
import { useQuery } from '@tanstack/react-query';
import { Check, Pencil, RefreshCw, Save, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { hostPortsQuery, saveHostPorts } from '@/api/ports';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { HelpHint } from '@/ui/HelpHint';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { RadioGroup } from '@/ui/RadioGroup';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { PortFault } from './Fault';
import type { HostPorts, HostPortsReview } from './presentation';

/** Inclusive TCP/UDP ranges are declaration data, not inferred firewall rules. */
export function parsePortRanges(value: string): HostPorts['ranges'] | null {
  const ranges: HostPorts['ranges'] = [];
  for (const piece of value.split(',')) {
    const match = /^\s*(\d+)\s*[-–]\s*(\d+)\s*$/.exec(piece);
    if (match === null) return null;
    const from = Number(match[1]); const to = Number(match[2]);
    if (from < 1 || to > 65535 || from > to || ranges.some((range) => from <= range.to && to >= range.from)) return null;
    ranges.push({ from, to });
  }
  return ranges.length === 0 ? null : ranges;
}

function HostPortForm({ id, data, onClose }: { id: string; data: HostPortsReview; onClose: () => void }) {
  const { t } = useTranslation();
  const initial = data.current ?? data.proposal;
  const [address, setAddress] = useState(initial.address);
  const [all, setAll] = useState(initial.bind === 'all');
  const [bind, setBind] = useState(initial.bind === 'all' ? '' : initial.bind);
  const [range, setRange] = useState(initial.ranges.map((entry) => `${entry.from}–${entry.to}`).join(', '));
  const [reviewed, setReviewed] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const ranges = parsePortRanges(range);
  const valid = address.trim() !== '' && (all || bind.trim() !== '') && ranges !== null;
  const save = async () => {
    if (busy || !reviewed || !data.can_edit || data.allocations === null || !clearing && !valid) return;
    setBusy(true); setError(null);
    try { await saveHostPorts(id, clearing ? null : { address: address.trim(), bind: all ? 'all' : bind.trim(), ranges: ranges ?? [] }, data.revision); onClose(); }
    catch (failed) { setError(failed); } finally { setBusy(false); }
  };
  const changed = () => { setReviewed(false); setClearing(false); };
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('ports.hostForm.title')} description={t('ports.hostForm.proposal')} dismissible={!busy} footer={<>
    <Button icon={<X aria-hidden />} onClick={onClose} pending={busy}>{t('ports.cancel')}</Button>
    {!reviewed && data.current !== null && <Button variant="danger-ghost" icon={<X aria-hidden />} onClick={() => { setClearing(true); setReviewed(true); }}>{t('ports.hostForm.clear')}</Button>}
    {reviewed ? <Button variant={clearing ? 'danger' : 'primary'} icon={<Save aria-hidden />} pending={busy} blocked={data.allocations === null} onClick={() => void save()}>{t(clearing ? 'ports.hostForm.clear' : 'ports.hostForm.save')}</Button> : <Button icon={<RefreshCw aria-hidden />} blocked={!valid} onClick={() => setReviewed(true)}>{t('ports.hostForm.review')}</Button>}
  </>}>
    <div className="flex flex-col gap-4">
      {!clearing && <><Input label={t('ports.hostForm.reached')} value={address} readOnly={reviewed} onChange={(event) => { setAddress(event.target.value); changed(); }} /><div className="flex items-center gap-1 text-sm font-medium">{t('ports.hostForm.bind')}<HelpHint term={t('ports.hostForm.bind')} what={t('ports.hostForm.allAbout')} /></div><RadioGroup<'all' | 'one'> label={t('ports.hostForm.bind')} value={all ? 'all' : 'one'} disabled={reviewed} onValueChange={(next) => { setAll(next === 'all'); changed(); }} variant="list" options={[{ value: 'all', ...choiceOption(t, 'hostBinding.all'), description: t('ports.hostForm.allAbout') }, { value: 'one', ...choiceOption(t, 'hostBinding.one') }]} />{!all && <Input label={t('ports.hostForm.one')} value={bind} readOnly={reviewed} onChange={(event) => { setBind(event.target.value); changed(); }} />}<Input label={t('ports.hostForm.range')} value={range} readOnly={reviewed} onChange={(event) => { setRange(event.target.value); changed(); }} hint={t('ports.rangeHint')} error={ranges === null ? t('ports.hostFormInvalid') : undefined} /></>}
      {reviewed && <div className="rounded-lg border border-warning/40 bg-warning-subtle p-3"><h3 className="text-sm font-semibold">{t('ports.hostForm.affected')}</h3>{data.allocations === null ? <p className="mt-2 text-sm">{t('ports.hostForm.missingReview')}</p> : <ul className="mt-2 flex flex-col gap-1 text-sm">{data.allocations.map((allocation) => <li key={allocation.opening_id} className="flex flex-wrap gap-x-3 gap-y-1"><span>{allocation.environment.name}</span><span>{t('ports.portNumber',{port:allocation.port})}</span><span>{t(`ports.transport.${allocation.transports}`)}</span></li>)}</ul>}<p className="mt-2 text-sm">{t(clearing || initial.bind !== (all ? 'all' : bind) ? 'ports.hostForm.closed' : 'ports.hostForm.kept')}</p></div>}
      <p className="text-sm text-text-muted">{t('ports.hostForm.noFirewall')}</p><PortFault error={error} />
    </div>
  </Dialog>;
}

export function HostPortsDeclaration({ hostId }: { hostId: string }) {
  const { t } = useTranslation(); const [editing, setEditing] = useState(false);
  const query = useQuery(hostPortsQuery(hostId));
  if (query.isPending) return <SkeletonRows rows={2} />;
  if (query.isError) return <PortFault error={query.error} onRefresh={() => void query.refetch()} />;
  const data = query.data;
  return <section aria-label={t('ports.hostForm.title')} className="flex flex-col gap-3 rounded-lg border border-border p-4" data-testid="host-ports">
    <div className="flex flex-wrap items-center justify-between gap-2"><h2 className="font-semibold">{t('ports.hostForm.title')}</h2>{data.can_edit && <Button size="sm" icon={<Pencil aria-hidden />} onClick={() => setEditing(true)}>{t('ports.hostForm.edit')}</Button>}</div>
    {data.current === null ? <p className="text-sm text-text-muted">{t('ports.hostForm.none')}</p> : <dl className="flex flex-col gap-2 text-sm"><div><dt className="text-text-muted">{t('ports.hostForm.reached')}</dt><dd className="break-all font-mono">{data.current.address}</dd></div><div><dt className="text-text-muted">{t('ports.hostForm.bind')}</dt><dd>{data.current.bind === 'all' ? t('ports.hostForm.all') : data.current.bind}</dd></div><div><dt className="text-text-muted">{t('ports.hostForm.range')}</dt><dd>{data.current.ranges.map((range) => `${range.from}–${range.to}`).join(', ')}</dd></div></dl>}
    {data.acknowledged === null && data.current !== null ? <p className="text-xs text-text-muted">{t('ports.hostAckUnknown')}</p> : data.acknowledged === false ? <InlineAlert tone="info">{t('ports.hostForm.pending')}</InlineAlert> : data.acknowledged === true && data.current !== null && <p className="flex items-center gap-1 text-xs text-text-muted"><Check size={16} aria-hidden />{t('ports.hostAck')}</p>}
    {data.sources !== undefined && <p className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{t('ports.proposalSources', { address: data.sources.incoming_address, listen: data.sources.listen, ranges: data.sources.ranges })}</span>{data.as_of !== undefined && <RelativeTime at={data.as_of} />}</p>}
    {data.limits !== undefined && <dl aria-label={t('ports.localCeiling')} className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 break-all text-xs text-text-muted"><dt>{t('ports.hostForm.reached')}</dt><dd>{data.limits.incoming_address ?? '—'}</dd><dt>{t('ports.hostForm.bind')}</dt><dd>{data.limits.listen==='all'?t('ports.hostForm.all'):data.limits.listen ?? '—'}</dd><dt>{t('ports.hostForm.range')}</dt><dd>{data.limits.ranges?.map((range) => `${range.from}–${range.to}`).join(', ') ?? '—'}</dd></dl>}
    {data.reason !== null && <InlineAlert tone="warning">{data.reason}</InlineAlert>}
    {editing && <HostPortForm id={hostId} data={data} onClose={() => setEditing(false)} />}
  </section>;
}
