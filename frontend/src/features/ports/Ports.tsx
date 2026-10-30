/** The same Ports snapshot on an environment page and beside a thread, without opening anything on selection. */
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ExternalLink, Network, Pencil, Plus, RefreshCw, Terminal } from 'lucide-react';
import { type KeyboardEvent, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { GuideLink } from '@/features/help/GuideLink';
import { ActionIcon } from '@/ui/actions';
import { useViewChoice } from '@/lib/viewChoices';
import { ApiError } from '@/api/client';
import { environmentKey, environmentsKey } from '@/api/environments';
import { enterPort, portsQuery, readPort, refreshPorts } from '@/api/ports';
import type { Environment } from '@/generated/api-types';
import { OpenEgressNotice } from '@/features/resources/environments/Network';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { StateLine } from '@/ui/StateLine';
import { PortConnections } from './Connections';
import { PortCopy } from './Copy';
import { PortEntries } from './Entries';
import { PortFault } from './Fault';
import { LetMoreIn } from './OpenDialog';
import { OpeningRow } from './Opening';
import { PortsGrants } from './Grants';
import { PortsPolicy } from './Policy';
import { PortRequests } from './Request';
import { ProgramDialog } from './Program';
import type { EnvironmentPort, EnvironmentPorts, PortsBrowser } from './presentation';

const DROP_LABELS = { oversized: 'ports.dropOversized', full: 'ports.dropFull', senders: 'ports.dropSenders', local_send: 'ports.dropSend', link_lost: 'ports.dropLink' } as const;

export function PortsPanel({ environment, browser, initialPort }: { environment: Environment; browser?: PortsBrowser; initialPort?: number }) {
 const { t } = useTranslation();
 const cache = useQueryClient();
 const query = useQuery(portsQuery(environment.id));
 const refresh = useMutation({ mutationFn: () => refreshPorts(environment.id), onSuccess: (data) => {
  cache.setQueryData(portsQuery(environment.id).queryKey, data);
  void cache.invalidateQueries({ queryKey: environmentKey(environment.id) });
  void cache.invalidateQueries({ queryKey: environmentsKey });
 } });
 if (query.isError || refresh.error instanceof ApiError && (refresh.error.status === 403 || refresh.error.status === 404)) return <PortFault error={query.error ?? refresh.error} onRefresh={() => void query.refetch()} />;
 if (query.isPending) return <div data-testid="ports-loading"><StateLine compact role="status" message={t('ports.waitingDiscovery')} action={<Button size="sm" icon={<RefreshCw aria-hidden />} pending={refresh.isPending} onClick={() => refresh.mutate()}>{t('ports.refresh')}</Button>} /><PortFault error={refresh.error} /></div>;
 const changed = () => {
  void cache.invalidateQueries({ queryKey: portsQuery(environment.id).queryKey });
  void cache.invalidateQueries({ queryKey: environmentKey(environment.id) });
 };
 return <PortsSnapshot environment={environment} data={query.data} browser={browser} initialPort={initialPort} refreshing={refresh.isPending} onRefresh={() => refresh.mutate()} onChanged={changed} error={refresh.error} />;
}

function PortRow({ port, environment, data, browser, onChanged }: { port: EnvironmentPort; environment: Environment; data: EnvironmentPorts; browser?: PortsBrowser; onChanged: () => void }) {
 const { t } = useTranslation(); const [lettingIn, setLettingIn] = useState(false); const [helper, setHelper] = useState(false); const [program, setProgram] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null); const [oldLink, setOldLink] = useState<EnvironmentPorts['openings'][number] | undefined>();
 const [listenersOpen, setListenersOpen] = useViewChoice<boolean>('ports', 'listeners', false, `${environment.id}:${port.port}`);
 const manage = environment.access.permissions.includes('ports.manage');
 const firstListener = port.listeners.at(0);
 const enter = async () => { if (busy) return; setBusy(true); setError(null); const child = window.open('about:blank', '_blank'); if (child !== null) child.opener = null; try { const reply = await enterPort(environment.id, port.port); if (child !== null) child.location.replace(reply.url); else window.location.assign(reply.url); } catch (failed) { child?.close(); setError(failed); } finally { setBusy(false); } };
 const rowKeys = (event: KeyboardEvent<HTMLElement>) => { if (event.target !== event.currentTarget) return; if (event.key === 'Enter') { event.preventDefault(); const details = event.currentTarget.querySelector('details'); if (details !== null) details.open = !details.open; } if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return; const rows = [...(event.currentTarget.parentElement?.querySelectorAll<HTMLElement>('[data-port-row]') ?? [])]; const index = rows.indexOf(event.currentTarget); rows[index + (event.key === 'ArrowDown' ? 1 : -1)]?.focus(); event.preventDefault(); };
 const http = port.listeners.length === 0 || port.listeners.some((listener) => listener.protocol === 'tcp');
 return <article tabIndex={0} data-port-row={port.port} data-testid={`port-${port.port}`} onKeyDown={rowKeys} className="@container min-w-0 rounded-lg border border-border bg-surface p-3 outline-offset-2">
  <div className="flex flex-col gap-2 @3xl:flex-row @3xl:items-start @3xl:justify-between">
   <div className="min-w-0 flex-1"><h3 className="break-all font-mono text-sm font-semibold">{port.url !== null && !port.internal ? port.url : t('ports.portNumber', { port: port.port })}</h3>{port.url !== null && !port.internal && <p className="mt-1 text-xs text-text-muted">{t('ports.portNumber', { port: port.port })}</p>}</div>
   {!port.internal && port.url !== null && http && <div className="flex flex-wrap gap-2"><Button size="sm" icon={<ExternalLink aria-hidden />} blocked={!port.can.enter} pending={busy} onClick={() => void enter()}>{t('ports.open')}</Button><PortCopy value={port.url} label={t('ports.copyAddress')} /></div>}
  </div>
  <p className="mt-2 text-sm text-text-muted">{port.internal ? t('ports.internalAbout') : t('ports.baselineAudience', { environment: environment.name })}</p>
  {!port.internal && port.openings.some((opening) => opening.desired) && <ul className="mt-1 flex flex-wrap gap-x-3 gap-y-1 break-words text-sm">{port.openings.filter((opening) => opening.desired).map((opening) => <li key={opening.id}>{opening.way === 'place' ? opening.place?.name ?? t('ports.hiddenPlace') : opening.way === 'link' ? opening.label === undefined || opening.label === '' ? t('ports.way.link') : opening.label : opening.url ?? t('ports.way.host')}</li>)}</ul>}
  {!listenersOpen && firstListener?.process && <p className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{firstListener.process.name}</span><span>{firstListener.process.thread?.name ?? t('ports.otherWork')}</span><span>{t('ports.firstSeen')} <RelativeTime at={firstListener.first_seen_at} /></span></p>}
  <div className="mt-2 flex flex-wrap items-center gap-2"><span className="rounded-full border border-border bg-bg-subtle px-2 py-1 text-xs">{port.internal ? t('ports.internal') : port.state === 'listening' && port.listeners.every((listener) => listener.protocol === 'udp') ? t('ports.boundUdp') : t(`ports.portState.${port.state}`)}</span>{port.reason !== null && <span className="text-xs text-warning-text">{port.reason}</span>}</div>
  {!port.internal && <div className="mt-3 flex flex-wrap gap-2">
   {port.url === null && http && <Button size="sm" icon={<ExternalLink aria-hidden />} blocked={!port.can.here || !environment.access.permissions.includes('browser.manage') || browser?.reason !== null} onClick={() => browser?.open(port)}>{t('ports.openHere')}</Button>}
   <Button size="sm" icon={<Terminal aria-hidden />} blocked={!port.can.enter} action="portHelper" onClick={() => setHelper(true)}>{t('craftJoined.showHelper')}</Button>
   {manage && <Button size="sm" icon={<Plus aria-hidden />} blocked={!port.can.let_more_in} onClick={() => { setOldLink(undefined); setLettingIn(true); }}>{t('ports.letMoreIn')}</Button>}
   {manage && <Button size="sm" icon={<Pencil aria-hidden />} blocked={port.revision === undefined} onClick={() => setProgram(true)}>{t('ports.editProgram')}</Button>}
  </div>}
  {!port.internal && port.program !== undefined && <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-xs text-text-muted"><dt>{t('ports.scheme')}</dt><dd>{port.program.scheme.toUpperCase()}</dd><dt>{t('ports.hostHeader')}</dt><dd>{port.program.host_header ?? t('ports.passHost')}</dd>{port.program.proxy_protocol===true && <><dt>{t('ports.proxyHeader')}</dt><dd>{t('common.on')}</dd></>}</dl>}
  {port.as_of !== undefined && <p className="mt-1 text-xs text-text-muted">{t('ports.stateAsOf')} <RelativeTime at={port.as_of} /></p>}
  {port.url === null && !port.internal && <p className="mt-2 text-xs text-text-muted">{t('ports.noBrowserName')}{http && (browser?.reason ?? (browser === undefined ? ` ${t('ports.browserMissing')}` : ''))}</p>}
  {!http && <p className="mt-2 text-xs text-text-muted">{t('ports.udpHttp')}</p>}
  <details open={listenersOpen} onToggle={(event) => setListenersOpen(event.currentTarget.open)} className="mt-3"><summary className="cursor-pointer text-sm text-text-muted inline-flex items-center gap-2"><ActionIcon action={listenersOpen ? 'hide' : 'show'} />{t('ports.listeners')}</summary><div className="mt-2 flex flex-col gap-2">
   {port.listeners.length === 0 && <EmptyState compact title={t(port.discovery === 'offline' ? 'ports.offline' : port.discovery === 'unsupported' ? 'ports.unsupported' : 'ports.empty')} />}
   {port.listeners.map((listener) => <div key={listener.identity} className="min-w-0 border-l-2 border-border pl-3 text-sm"><p className="font-medium">{t('ports.listener', { transport: listener.protocol.toUpperCase(), port: listener.port })}</p><p>{listener.process?.name ?? t('ports.unknownProgram')}</p>{listener.binding === 'bound_unknown' && <p className="text-xs text-text-muted">{t('ports.boundUdp')}</p>}<p className="break-all font-mono text-xs text-text-muted">{listener.local_address}</p><p className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{listener.process?.thread?.name ?? t('ports.otherWork')}</span><span>{t('ports.firstSeen')} <RelativeTime at={listener.first_seen_at} /></span></p>{listener.unavailable_reason !== null && <p className="text-xs text-warning-text">{listener.unavailable_reason}</p>}</div>)}
  </div></details>
  {!port.internal && <div className="mt-3"><PortConnections environmentId={environment.id} port={port.port} count={port.counts} manage={manage} /></div>}
  {port.openings.length > 0 && <div className="mt-3 flex flex-col gap-2">{port.openings.map((opening) => <OpeningRow key={opening.id} opening={opening} port={port} data={data} manage={manage} onChanged={onChanged} onError={setError} onNewLink={() => { setOldLink(opening); setLettingIn(true); }} />)}</div>}
  <div className="mt-2"><PortFault error={error} /></div>
  {lettingIn && <LetMoreIn port={port} environment={environment} data={data} oldLink={oldLink} onClose={() => setLettingIn(false)} onChanged={onChanged} onError={setError} />}
  {program && <ProgramDialog environmentId={environment.id} port={port} onClose={() => setProgram(false)} onChanged={onChanged} />}
  <Dialog open={helper} onOpenChange={setHelper} title={t('ports.withHelper')} description={t('ports.forwardAbout')}>{port.entries === undefined ? <><code className="block break-all rounded-md bg-bg-muted p-3 text-sm">{port.helper_command}</code><PortCopy value={port.helper_command} label={t('ports.copyCommand')} /></> : <PortEntries entries={port.entries.filter((entry) => entry.method === 'helper')} />}<p className="mt-3 text-sm text-text-muted">{t('ports.forwardPending')}</p><p className="mt-2 text-sm text-text-muted">{t('ports.phoneForward')}</p></Dialog>
 </article>;
}

export function PortsSnapshot({ environment, data, browser, initialPort, onRefresh, onChanged, refreshing = false, error }: { environment: Environment; data: EnvironmentPorts; browser?: PortsBrowser; initialPort?: number; onRefresh: () => void; onChanged?: () => void; refreshing?: boolean; error?: unknown }) {
 const { t } = useTranslation(); const [number, setNumber] = useState(initialPort === undefined ? '' : String(initialPort)); const [read, setRead] = useState<EnvironmentPort | null>(null); const [readError, setReadError] = useState<unknown>(null); const [reading, setReading] = useState(false);
 const old = data.snapshot.discovery === 'offline'; const unsupported = data.snapshot.discovery === 'unsupported';
 const ports = read !== null && !data.ports.some((port) => port.port === read.port) ? [...data.ports, read] : data.ports;
 const validPort = /^\d+$/.test(number) && Number(number) >= 1 && Number(number) <= 65535;
 const showPort = async () => { if (!validPort || reading) return; setReading(true); setReadError(null); try { setRead(await readPort(environment.id, Number(number))); } catch (failed) { setReadError(failed); } finally { setReading(false); } };
 const changed = () => {
  (onChanged ?? onRefresh)();
  if (read !== null) void readPort(environment.id, read.port).then(setRead, (failed: unknown) => setReadError(failed));
 };
 useEffect(() => { if (initialPort === undefined) return; let active = true; void readPort(environment.id, initialPort).then((target) => { if (active) { setNumber(String(initialPort)); setRead(target); } }, (failed: unknown) => { if (active) setReadError(failed); }); return () => { active = false; }; }, [environment.id, initialPort]);
 return <section aria-label={t('ports.title')} className="flex min-w-0 flex-col gap-4 p-3 text-text" data-testid="ports-panel">
  <header className="flex flex-wrap items-start justify-between gap-2"><div className="min-w-0"><h2 className="flex flex-wrap items-center gap-2 text-lg font-semibold"><Network aria-hidden /><Link to="/resources/environments/$id" params={{ id: environment.id }} search={{ tab: 'ports' }} className="hover:underline">{t('ports.inEnvironment', { name: environment.name })}</Link></h2><p className="flex flex-wrap gap-x-2 text-xs text-text-muted">{t('ports.generation', { generation: data.snapshot.generation })}<span>{t('ports.observed')} <RelativeTime at={data.snapshot.as_of} /></span></p></div><Button size="sm" icon={<RefreshCw aria-hidden />} pending={refreshing} onClick={onRefresh}>{t('ports.refresh')}</Button></header>
  <PortFault error={error} onRefresh={onRefresh} />
  <GuideLink guide="let-agents-open-ports" target={{ kind: 'environment', id: environment.id }} />
  {data.snapshot.udp_dropped !== undefined && Object.values(data.snapshot.udp_dropped).some((count) => count > 0) && <InlineAlert tone="warning" title={t('ports.udpDrops')}><div className="flex flex-wrap gap-x-4 gap-y-1">{(Object.entries(data.snapshot.udp_dropped) as [keyof typeof DROP_LABELS, number][]).filter(([, count]) => count > 0).map(([kind, count]) => <span key={kind}>{t(DROP_LABELS[kind])}: {count}</span>)}</div></InlineAlert>}
  {old && <InlineAlert tone="warning" title={t('ports.old')}>{data.snapshot.reason ?? t('ports.offline')}</InlineAlert>}
  {unsupported && <InlineAlert tone="warning" title={t('ports.unsupported')}>{data.snapshot.reason}</InlineAlert>}
  {!data.browser_available && <InlineAlert tone="info">{t('ports.noBrowserName')}</InlineAlert>}
  <OpenEgressNotice network={environment.network} />
  <PortsPolicy data={data} />
  <PortsGrants environmentId={environment.id} capabilities={data.capabilities} onChanged={changed} />
  <form className="flex flex-wrap items-end gap-2" onSubmit={(event) => { event.preventDefault(); void showPort(); }}><Input type="number" min={1} max={65535} label={t('ports.findPort')} value={number} onChange={(event) => setNumber(event.target.value)} fieldClassName="min-w-0 flex-1 sm:max-w-48" /><Button type="submit" size="md" icon={<Network aria-hidden />} pending={reading} blocked={!validPort}>{t('ports.showPort')}</Button></form><PortFault error={readError} />
  {!old && !unsupported && data.listeners.length === 0 && <EmptyState compact title={t('ports.empty')} description={ports.length === 0 ? t('ports.stableBeforeListening') : undefined} />}
  <div className="flex flex-col gap-3">{ports.map((port) => <PortRow key={port.port} port={port} environment={environment} data={data} browser={browser} onChanged={changed} />)}</div>
  <PortRequests environment={environment} onChanged={changed} />
 </section>;
}
