/** Safe opening rows: audience before carriers, reached host address first, and live copy only after bind. */
import { Link as LinkIcon, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { closePort } from '@/api/ports';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { RelativeTime } from '@/ui/RelativeTime';
import { liveAddress, outsideRange } from './model';
import { PortConnections } from './Connections';
import { PortEntries } from './Entries';
import { PortCopy } from './Copy';
import { PortFault } from './Fault';
import type { EnvironmentPort, EnvironmentPorts, PortOpening } from './presentation';

/** Who or what let this opening in: the deciding person, or the grant, template default or place policy that let an agent open it without asking. */
export function OpeningAdmission({ opening }: { opening: PortOpening }) {
 const { t } = useTranslation();
 const admission = opening.admission ?? null; const decider = opening.allowed_by ?? null;
 if (admission === null) return decider === null ? null : <p className="text-xs text-text-muted">{t('ports.allowedBy', { name: decider.display_name })}</p>;
 const name = admission.by?.display_name ?? t('ports.unreported');
 return <p className="text-xs text-text-muted">{decider !== null ? t('ports.admission.decidedGrant', { name: decider.display_name }) : admission.source === 'grant' ? t('ports.admission.grant', { name }) : admission.source === 'template' ? t('ports.admission.template') : t('ports.admission.policy', { place: admission.scope?.name ?? t('ports.system') })}</p>;
}

export function OpeningRow({ opening, port, data, manage, onChanged, onError, onNewLink }: { opening: PortOpening; port: EnvironmentPort; data: EnvironmentPorts; manage: boolean; onChanged: () => void; onError: (error: unknown) => void; onNewLink?: () => void }) {
 const { t } = useTranslation(); const [closing, setClosing] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null);
 const address = liveAddress(opening);
 const close = async () => { if (busy) return; setBusy(true); setError(null); try { await closePort(opening); onChanged(); setClosing(false); } catch (failed) { setError(failed); onError(failed); } finally { setBusy(false); } };
 return <section className="min-w-0 rounded-lg bg-bg-subtle p-3" aria-label={t('ports.opening')} data-testid={`opening-${opening.way}`}>
  {address !== null && <div className="mb-2 flex flex-wrap items-center gap-2"><strong className="break-all font-mono">{address}</strong><span className="text-xs text-text-muted">{t(`ports.transport.${opening.transports}`)}</span><PortCopy value={address} label={t('ports.copyAddress')} /></div>}
  <div className="flex flex-wrap items-center gap-2 text-sm"><strong>{opening.way === 'place' ? t('ports.outsidePlace', { place: opening.place?.name ?? t('ports.hiddenPlace') }) : t(`ports.way.${opening.way}`)}</strong><span className="rounded-full border border-border px-2 py-1 text-xs">{t(`ports.state.${opening.state}`)}</span>{opening.fixed_host_port && <span>{t('ports.fixed')}</span>}</div>
  {opening.way === 'place' && port.url !== null && <p className="mt-1 break-all font-mono text-xs text-text-muted">{port.url}</p>}
  <p className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{t('ports.askedBy', { name: opening.requested_by_thread?.name ?? opening.requested_by?.display_name ?? opening.opened_by?.display_name ?? t('ports.removedRequester') })}</span><RelativeTime at={opening.opened_at} /></p>
  <OpeningAdmission opening={opening} />
  {opening.until !== null && <p className="text-sm text-text-muted">{t('ports.until')} <RelativeTime at={opening.until} /></p>}
  {opening.reason !== null && <p className="mt-1 text-sm text-warning-text">{opening.reason}</p>}
  {outsideRange(opening, data.host_ports) && <p className="text-xs text-warning-text">{t('ports.retained')}</p>}
  {opening.state === 'opening' && <p className="text-xs text-text-muted">{t('ports.liveOnly')}</p>}
  {opening.entries !== undefined && <div className="mt-2"><PortEntries entries={opening.entries.filter((entry) => entry.method !== 'host')} /></div>}
  {opening.entries === undefined && opening.way !== 'host' && <p className="mt-2 text-xs text-text-muted">{t('ports.carriers')}: {opening.link_carriers === 'helper' || port.url === null ? t('ports.withHelper') : opening.link_carriers === 'browser' ? t('ports.inBrowser') : [t('ports.inBrowser'),t('ports.withHelper')].join(t('startFlow.separator'))}</p>}
  <div className="mt-2 flex flex-wrap gap-2">
   {opening.way === 'link' && manage && onNewLink !== undefined && <Button size="sm" icon={<LinkIcon aria-hidden />} onClick={onNewLink}>{t('ports.newLink')}</Button>}
   {opening.can.close && opening.state !== 'closed' && opening.state !== 'expired' && <Button size="sm" variant="danger-ghost" icon={<X aria-hidden />} onClick={() => setClosing(true)}>{t('ports.closeOpening')}</Button>}
  </div>
  {opening.way === 'link' && <p className="mt-2 text-xs text-text-muted">{t('ports.linkNoRecovery')}</p>}
  <div className="mt-2"><PortConnections opening={opening} manage={manage} /></div><PortFault error={error} />
  <Dialog open={closing} onOpenChange={setClosing} title={t('ports.closeTitle')} description={t('ports.closeAbout')} dismissible={!busy} footer={<><Button onClick={() => setClosing(false)}>{t('ports.cancel')}</Button><Button variant="danger" icon={<X aria-hidden />} pending={busy} onClick={() => void close()}>{t('ports.closeOpening')}</Button></>}><p className="text-sm text-text-muted">{t('ports.portNumber',{port:opening.port})} ({t(`ports.way.${opening.way}`)})</p><PortFault error={error} /></Dialog>
 </section>;
}
