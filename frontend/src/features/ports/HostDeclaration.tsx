/** The host's three explicit port fields, saved at once; only a change that closes open ports says so, in one line with the ports on Show. */
import { useQuery } from '@tanstack/react-query';
import { Check, Pencil, Save, X } from 'lucide-react';
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

/** Open ports that a change closes: one line, their environments and ports on Show; unknown openings are said as a rule. */
function ClosingLine({ allocations }: { allocations: HostPortsReview['allocations'] }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (allocations === null) return <p className="text-sm text-warning-text">{t('ports.hostForm.closed')}</p>;
  if (allocations.length === 0) return null;
  return <div className="flex flex-col gap-2 text-sm">
    <p className="flex flex-wrap items-center gap-2"><span className="text-warning-text">{t('ports.hostForm.closes', { count: allocations.length })}</span>
      <Button size="sm" variant="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>{t(open ? 'actions.hide.label' : 'actions.show.label')}</Button></p>
    {open && <ul className="flex flex-col gap-1">{allocations.map((allocation) => <li key={allocation.opening_id} className="flex flex-wrap gap-x-3 gap-y-1"><span>{allocation.environment.name}</span><span>{t('ports.portNumber',{port:allocation.port})}</span><span>{t(`ports.transport.${allocation.transports}`)}</span></li>)}</ul>}
  </div>;
}

function HostPortForm({ id, data, onClose }: { id: string; data: HostPortsReview; onClose: () => void }) {
  const { t } = useTranslation();
  const initial = data.current ?? data.proposal;
  const [address, setAddress] = useState(initial.address);
  const [all, setAll] = useState(initial.bind === 'all');
  const [bind, setBind] = useState(initial.bind === 'all' ? '' : initial.bind);
  const [range, setRange] = useState(initial.ranges.map((entry) => `${entry.from}–${entry.to}`).join(', '));
  const [busy, setBusy] = useState(false);
  const [clearing, setClearing] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const ranges = parsePortRanges(range);
  const valid = address.trim() !== '' && (all || bind.trim() !== '') && ranges !== null;
  // A smaller range keeps acknowledged listeners until they close; only another Listen on or clearing closes them now.
  const closesOnSave = data.current !== null && initial.bind !== (all ? 'all' : bind.trim());
  const clearCloses = data.allocations === null || data.allocations.length > 0;
  const edit = <T,>(set: (value: T) => void) => (value: T) => { set(value); setClearing(false); };
  const save = async (clearing: boolean) => {
    if (busy || !data.can_edit || !clearing && !valid) return;
    setBusy(true); setError(null);
    try { await saveHostPorts(id, clearing ? null : { address: address.trim(), bind: all ? 'all' : bind.trim(), ranges: ranges ?? [] }, data.revision); onClose(); }
    catch (failed) { setError(failed); } finally { setBusy(false); }
  };
  return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('ports.hostForm.title')} description={t('ports.hostForm.proposal')} dismissible={!busy} footer={<>
    <Button icon={<X aria-hidden />} onClick={onClose} pending={busy}>{t('ports.cancel')}</Button>
    {data.current !== null && <Button variant={clearing ? 'danger' : 'danger-ghost'} icon={<X aria-hidden />} pending={busy} onClick={() => { if (clearCloses && !clearing) setClearing(true); else void save(true); }}>{t('ports.hostForm.clear')}</Button>}
    <Button variant="primary" icon={<Save aria-hidden />} pending={busy} blocked={!valid} onClick={() => void save(false)}>{t('ports.hostForm.save')}</Button>
  </>}>
    <div className="flex flex-col gap-4">
      <Input label={t('ports.hostForm.reached')} value={address} onChange={(event) => edit(setAddress)(event.target.value)} /><div className="flex items-center gap-1 text-sm font-medium">{t('ports.hostForm.bind')}<HelpHint term={t('ports.hostForm.bind')} what={t('ports.hostForm.allAbout')} /></div><RadioGroup<'all' | 'one'> label={t('ports.hostForm.bind')} value={all ? 'all' : 'one'} onValueChange={(next) => edit(setAll)(next === 'all')} variant="list" options={[{ value: 'all', ...choiceOption(t, 'hostBinding.all'), description: t('ports.hostForm.allAbout') }, { value: 'one', ...choiceOption(t, 'hostBinding.one') }]} />{!all && <Input label={t('ports.hostForm.one')} value={bind} onChange={(event) => edit(setBind)(event.target.value)} />}<Input label={t('ports.hostForm.range')} value={range} onChange={(event) => edit(setRange)(event.target.value)} hint={t('ports.rangeHint')} error={ranges === null ? t('ports.hostFormInvalid') : undefined} />
      {(clearing || closesOnSave) && <ClosingLine allocations={data.allocations} />}
      <p className="text-sm text-text-muted">{t('ports.hostForm.noFirewall')}</p><PortFault error={error} />
    </div>
  </Dialog>;
}

export function HostPortsDeclaration({ hostId }: { hostId: string }) {
  const { t } = useTranslation(); const [editing, setEditing] = useState(false);
  const query = useQuery(hostPortsQuery(hostId));
  if (query.isPending) return <SkeletonRows read={query} rows={2} />;
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
