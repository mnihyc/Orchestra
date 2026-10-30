/** Deliberate opening reviews: exact observed target, audience, bounded secret reply and stored host candidate. */
import { useQuery } from '@tanstack/react-query';
import { Check, ExternalLink, Link as LinkIcon, RefreshCw, Users, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { ApiError } from '@/api/client';
import { createOpening, preflightOpening, previewPlace, previewPublication } from '@/api/ports';
import type { Environment, NameHit, PortLinkDelivery, PortOpeningPlan } from '@/generated/api-types';
import { useNow } from '@/lib/time';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { Dialog } from '@/ui/Dialog';
import { InlineAlert } from '@/ui/InlineAlert';
import { Input } from '@/ui/Input';
import { DateField } from '@/ui/DateField';
import { RadioGroup } from '@/ui/RadioGroup';
import { OnceLink } from './Entries';
import { PortFault } from './Fault';
import { DEFAULT_HOST_TRANSPORTS, validExpiry } from './model';
import { PlaceChoice } from './PlaceChoice';
import type { EnvironmentPort, EnvironmentPorts, HostTransports, LinkCarriers, PortOpening, PortPublishPlan } from './presentation';
import { DEFAULT_PROGRAM, ProgramFields, validProgram } from './Program';
import { methodsFor, openingView, programFor, transportsFor } from './wire';

type Audience = PortOpening['way'];
const ICON = { place: Users, link: LinkIcon, host: ExternalLink } as const;
export function LetMoreIn({ port, environment, data, oldLink, onClose, onChanged, onError }: { port: EnvironmentPort; environment: Environment; data: EnvironmentPorts; oldLink?: PortOpening; onClose: () => void; onChanged: () => void; onError: (error: unknown) => void }) {
 const { t, i18n } = useTranslation(); const now = useNow(); const mounted = useRef(true);
 useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
 const [way, setWay] = useState<Audience>(oldLink === undefined ? 'place' : 'link');
 const [chosen, setChosen] = useState<NameHit | null>(null);
 const [program, setProgram] = useState(port.program ?? DEFAULT_PROGRAM);
 const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null); const [expiry, setExpiry] = useState(''); const [fixed, setFixed] = useState(''); const [label, setLabel] = useState('');
 const [transports, setTransports] = useState<HostTransports>(DEFAULT_HOST_TRANSPORTS);
 const httpAvailable = port.url !== null;
 const [carriers, setCarriers] = useState<LinkCarriers>(httpAvailable ? 'both' : 'helper');
 const [hostPlan, setHostPlan] = useState<PortPublishPlan | null>(null); const [linkPlan, setLinkPlan] = useState<PortOpeningPlan | null>(null); const [result, setResult] = useState<PortOpening | null>(null);
 // A direct credential reply lives only while this dialog is mounted.
 const [delivery, setDelivery] = useState<PortLinkDelivery | null>(null); const [lost, setLost] = useState(false); const [priorLink, setPriorLink] = useState<PortOpening | null>(oldLink ?? null); const [closeOld, setCloseOld] = useState(false);
 const [clientId, setClientId] = useState(() => crypto.randomUUID());
 const attempt = useRef<string | null>(null);
 const place = useQuery({ queryKey: ['port-place-review', environment.id, port.port, chosen?.id, program], queryFn: () => previewPlace(environment.id, port.port, chosen?.id ?? '', program), enabled: chosen !== null && way === 'place' && validProgram(program) && result === null, retry: false });
 const allowed = port.can.let_more_in && !port.internal && (way === 'place' ? data.can.open_place : way === 'link' ? data.can.make_link : data.can.publish);
 const max = data.policy.link_max_secs.value;
 const validUntil = validExpiry(expiry, max, now);
 const fixedPort = fixed === '' ? undefined : Number(fixed);
 const validFixed = fixedPort === undefined || Number.isInteger(fixedPort) && fixedPort >= 1 && fixedPort <= 65535 && data.host_ports?.ranges.some((range) => fixedPort >= range.from && fixedPort <= range.to) === true;
 const invalidate = () => { setHostPlan(null); setLinkPlan(null); attempt.current = null; setClientId(crypto.randomUUID()); };
 const call = async (operation: () => Promise<void>, creating = false) => { if (busy) return; setBusy(true); setError(null); try { await operation(); } catch (failed) { if (!mounted.current) return; setError(failed); onError(failed); if (failed instanceof ApiError && failed.status === 409) invalidate(); if (creating && way === 'link' && (!(failed instanceof ApiError) || failed.status >= 500)) setLost(true); } finally { if (mounted.current) setBusy(false); } };
 const review = () => void call(async () => {
  if (!allowed || !validProgram(program)) return;
  if (way === 'host') { const plan = await previewPublication(environment.id, port.port, transports, fixedPort, program); if (mounted.current) setHostPlan(plan); }
  if (way === 'link') {
   const plan = await preflightOpening(environment.id, { port: port.port, kind: 'link', label, until: new Date(expiry).toISOString(), entry_methods: methodsFor(carriers), transports: carriers === 'browser' ? ['tcp'] : transportsFor(transports), program: programFor(program), reason: '', ...(priorLink?.desired !== true ? {} : { replacement_of: priorLink.id, close_replaced: closeOld }) });
   if (mounted.current) setLinkPlan(plan);
  }
 });
 const currentPlan = way === 'link' ? linkPlan : way === 'host' ? hostPlan : null;
 const planCurrent = currentPlan !== null && Date.parse(currentPlan.expires_at) > now;
 const create = () => void call(async () => {
  if (!allowed || !validProgram(program)) return;
  const preflight = lost ? attempt.current : way === 'place' ? place.data?.preflight_id : currentPlan?.preflight_id;
  if (preflight === undefined || preflight === null) return;
  attempt.current ??= preflight;
  const reply = await createOpening(environment.id, port.port, clientId, attempt.current);
  const opening = openingView(reply.opening);
  if (mounted.current) { setResult(opening); setDelivery(reply.link); if (way === 'link') setLost(reply.link?.url_once === null || reply.link === null); }
  onChanged();
 }, true);
 const newLink = () => { setPriorLink(result); invalidate(); setDelivery(null); setResult(null); setLost(false); setCloseOld(false); setError(null); };
 const Icon = ICON[way];
 const disabled = !allowed || !validProgram(program) || (way === 'place' ? chosen === null || place.data?.allowed !== true || place.isFetching : !planCurrent || currentPlan.blockers.length > 0);
 return <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={t('ports.letMoreIn')} description={<span className="break-all">{port.url ?? t('ports.portNumber', { port: port.port })}</span>} dismissible={!busy} footer={<>
  {way === 'host' && hostPlan !== null && result === null && <p className="w-full rounded-md bg-warning-subtle p-2 text-sm text-text">{hostPlan.address === '' ? hostPlan.audience : t('ports.hostAudience', { address: hostPlan.address })}</p>}
  <Button icon={<X aria-hidden />} pending={busy} onClick={onClose}>{t('ports.cancel')}</Button>
  {(way === 'host' || way === 'link') && result === null && !lost && <Button icon={<RefreshCw aria-hidden />} pending={busy} blocked={!allowed || !validProgram(program) || (way === 'host' ? !validFixed : !validUntil)} onClick={review}>{t(currentPlan === null ? 'ports.preview' : 'ports.reviewAgain')}</Button>}
  {lost && result !== null ? <Button icon={<LinkIcon aria-hidden />} onClick={newLink}>{t('ports.newLink')}</Button> : lost ? <Button icon={<RefreshCw aria-hidden />} pending={busy} blocked={!allowed} onClick={create}>{t('ports.checkReply')}</Button> : result === null && <Button variant="primary" icon={<Icon aria-hidden />} pending={busy} blocked={disabled} onClick={create}>{t(way === 'place' ? 'ports.confirmPlace' : way === 'link' ? 'ports.makeLink' : 'ports.confirmPublish')}</Button>}
 </>}><div className="flex min-w-0 flex-col gap-4 pb-2">
  {result === null && !lost && <RadioGroup<Audience> label={t('ports.audience')} value={way} onValueChange={(next) => { setWay(next); setError(null); invalidate(); }} variant="list" options={(['place','link','host'] as const).map((value) => { const Mark = ICON[value]; const enabled = value === 'place' ? data.can.open_place : value === 'link' ? data.can.make_link : data.can.publish; return { value, ...choiceOption(t, `portAudience.${value}`), icon: <Mark aria-hidden />, disabled: enabled ? false : data.reasons.find((reason) => reason.act === value)?.message ?? t('ports.notAllowed') }; })} />}
  {way === 'place' && <>
   {result === null && <PlaceChoice chosen={chosen} onChoose={(value) => { setChosen(value); attempt.current = null; setClientId(crypto.randomUUID()); }} />}
   {place.isFetching && chosen !== null && <p role="status" className="text-sm text-text-muted">{t('ports.pending')}</p>}
   {place.data !== undefined && <><p className="text-sm">{t('ports.placeAudience', { place: place.data.place.name })}</p><p className="text-sm text-text-muted">{place.data.viewers === null ? t('ports.countUnknown') : t('ports.placeCount', { count: place.data.viewers })}</p>{place.data.reason !== null && <p className="text-sm text-warning-text">{place.data.reason}</p>}</>}
   <PortFault error={place.error} /><p className="text-sm text-text-muted">{t('ports.placeUnchanged')}</p>
  </>}
  {way === 'link' && <>
   <p className="text-sm">{validUntil ? t('ports.linkAudience', { time: new Date(expiry).toLocaleString(i18n.language) }) : t('ports.linkAudienceBefore')}</p>
   {result === null && !lost && <>
    <Input label={t('ports.linkLabel')} value={label} onChange={(event) => { setLabel(event.target.value); invalidate(); }} />
    <DateField type="datetime-local" label={t('ports.expiry')} value={expiry} onValueChange={(value) => { setExpiry(value); invalidate(); }} required hint={typeof max === 'number' ? t('ports.expiryHint', { seconds: max, scope: data.policy.link_max_secs.source.node?.name ?? t('ports.system') }) : t('ports.maximumUnknown')} error={expiry !== '' && !validUntil ? t('ports.expiryInvalid') : undefined} />
    <RadioGroup<LinkCarriers> label={t('ports.carries')} value={carriers} onValueChange={(value) => { setCarriers(value); invalidate(); }} variant="list" options={[{ value:'both', ...choiceOption(t,'linkCarrier.both'), disabled:httpAvailable ? false : t('ports.noBrowserName') }, { value:'browser', ...choiceOption(t,'linkCarrier.browser'), disabled:httpAvailable ? false : t('ports.noBrowserName') }, { value:'helper', ...choiceOption(t,'linkCarrier.helper'), description:t('ports.carrierHint') }]} />
    {!httpAvailable && <p className="text-sm text-text-muted">{t(data.browser_available ? 'ports.udpHttp' : 'ports.noBrowserName')}</p>}
    {priorLink !== null && <Checkbox label={t('ports.closeOld')} checked={closeOld} onCheckedChange={(value) => { setCloseOld(value); invalidate(); }} />}
   </>}
   <p className="text-sm text-text-muted">{t('ports.linkWarning')}</p>
   {linkPlan !== null && result === null && <div className="rounded-lg border border-border p-3"><p className="text-sm font-medium">{linkPlan.audience.sentence}</p>{linkPlan.blockers.map((blocker) => <p key={blocker.code} className="text-sm text-danger-text">{blocker.message}</p>)}<p className="mt-2 flex items-center gap-1 text-xs text-text-muted"><Check size={16} aria-hidden />{t('ports.reviewedUntil', { time: new Date(linkPlan.expires_at).toLocaleString(i18n.language) })}</p></div>}
   {lost && <InlineAlert tone="warning" title={t('ports.linkLost')}>{t('ports.linkLostAbout')}</InlineAlert>}
   {delivery?.url_once !== null && delivery !== null && <OnceLink delivery={delivery} transports={carriers === 'browser' ? ['tcp'] : transportsFor(transports)} />}
  </>}
  {way === 'host' && <>
   <dl className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 break-all text-sm text-text-muted"><dt>{t('ports.hostForm.reached')}</dt><dd>{data.host_ports?.address ?? t('ports.unreported')}</dd><dt>{t('ports.hostForm.bind')}</dt><dd>{data.host_ports?.bind === 'all' ? t('ports.hostForm.all') : data.host_ports?.bind ?? t('ports.unreported')}</dd><dt>{t('ports.hostForm.range')}</dt><dd>{data.host_ports?.ranges.map((range) => `${range.from}–${range.to}`).join(', ') ?? t('ports.noRange')}</dd></dl>
   {result === null && <Input type="number" min={1} max={65535} label={t('ports.hostPort')} optional value={fixed} onChange={(event) => { setFixed(event.target.value); invalidate(); }} hint={t('ports.hostPortHint')} error={!validFixed ? t('ports.hostPortInvalid') : undefined} />}
   {hostPlan !== null && <div className="flex flex-col gap-2 rounded-lg border border-warning/40 bg-warning-subtle p-3"><strong className="break-all font-mono">{hostPlan.address}</strong><p className="flex flex-wrap gap-x-3 gap-y-1 text-sm"><span>{hostPlan.publisher?.name ?? hostPlan.host?.name}</span><span>{t(`ports.transport.${hostPlan.transports}`)}</span>{hostPlan.fixed && <span>{t('ports.fixed')}</span>}</p>{hostPlan.blockers.map((blocker) => <p key={blocker.code} className="text-sm text-danger-text">{blocker.message}</p>)}</div>}
   <p className="text-sm text-text-muted">{t('ports.hostWarning')}</p>
  </>}
  {result === null && !lost && way !== 'place' && (way !== 'link' || carriers !== 'browser') && <RadioGroup<HostTransports> label={t('ports.transports')} value={transports} onValueChange={(value) => { setTransports(value); invalidate(); }} variant="list" options={(['tcp_udp','tcp','udp'] as const).map((value) => ({ value, ...choiceOption(t,`transport.${value}`) }))} />}
  {result === null && !lost && <ProgramFields value={program} tcp={transports !== 'udp' || way === 'place' || carriers === 'browser'} onChange={(value) => { setProgram(value); invalidate(); }} affected={port.openings.filter((opening) => opening.desired).map((opening) => opening.label === undefined || opening.label === '' ? opening.place?.name ?? opening.host?.name ?? opening.id : opening.label)} />}
  {currentPlan !== null && !planCurrent && result === null && <InlineAlert tone="warning">{t('ports.candidateChanged')}</InlineAlert>}
  {result !== null && <InlineAlert tone={result.state === 'failed' || result.state === 'unavailable' || result.state === 'refused' ? 'danger' : 'info'} title={t(`ports.state.${result.state}`)}>{result.reason ?? t('ports.keepOpenings')}</InlineAlert>}
  <PortFault error={error} />
 </div></Dialog>;
}
