/** A Ports opening request's own card, including the explicit destination of an agent-requested link. */
import { useQuery } from '@tanstack/react-query';
import { Check, RefreshCw, Repeat, X } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ActionIcon } from '@/ui/actions';
import { decidePortRequest, portRequestsQuery, readPortApproval, requestView } from '@/api/ports';
import type { Approval, Environment } from '@/generated/api-types';
import { CardFrame, DecidedCard } from '@/features/approvals/Card';
import { useNow } from '@/lib/time';
import { useViewChoice } from '@/lib/viewChoices';
import { useSocketStatus } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { InlineAlert } from '@/ui/InlineAlert';
import { RelativeTime } from '@/ui/RelativeTime';
import { OnceLink, PortEntries } from './Entries';
import { PortFault } from './Fault';
import type { PortApproval } from './pending';
import type { PortRequest, PortRequestResult } from './presentation';
import { transportsFor } from './wire';

/** Keeps the ungenerated kind at one integration boundary; no credential reply is a list approval. */
export function asPortApproval(value: unknown): PortApproval | null {
  return typeof value === 'object' && value !== null && 'kind' in value && value.kind === 'port' ? value as PortApproval : null;
}
export function PortRequestCard({ request, live = true, onChanged, onSettled }: { request: PortRequest; live?: boolean; onChanged?: () => void; onSettled?: (request: PortRequest) => void }) {
  const { t, i18n } = useTranslation(); const now = useNow();
  const [forMeOnly, setForMeOnly] = useState(request.approval?.request.link_recipient === 'person'); const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null); const [result, setResult] = useState<PortRequestResult | null>(null); const [reviewed, setReviewed] = useState<PortRequest | null>(null);
  const [programOpen, setProgramOpen] = useViewChoice<boolean>('ports', 'requestProgram', false, request.id);
  const current = reviewed ?? request; const plan = current.approval?.request.plan;
  const stale = plan !== undefined && Date.parse(plan.expires_at) <= now;
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const refresh = async () => { if (busy) return; setBusy(true); setError(null); try { const approval = await readPortApproval(current.id); if (mounted.current) setReviewed(requestView(approval)); } catch (failed) { if (mounted.current) setError(failed); } finally { if (mounted.current) setBusy(false); } };
  const decide = async (decision: 'once' | 'grant' | 'deny') => {
    if (busy || !live || !current.can_decide || current.state !== 'pending' || decision !== 'deny' && stale) return;
    setBusy(true); setError(null);
    try { const reply = await decidePortRequest(current, decision, forMeOnly); if (mounted.current) setResult(reply); onSettled?.(reply.request); onChanged?.(); }
    catch (failed) { if (mounted.current) setError(failed); }
    finally { if (mounted.current) setBusy(false); }
  };
  const state = result?.request.state ?? current.state;
  // Granting the audience's capability lets later requests of this kind open without a card; offered where those deciding the card are its approvers.
  const canGrant = current.approval?.request.can_grant ?? false;
  const body = <div className="min-w-0" data-testid="port-request" data-approval={current.id}>
    <div className="flex flex-wrap items-center gap-2">{current.approval === undefined && <h3 className="text-sm font-semibold">{t('ports.requestTitle')}</h3>}<span className="rounded-full bg-bg-muted px-2 py-1 text-xs">{t(`ports.requestState.${state}`)}</span></div>
    <dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-sm"><dt>{t('approvals.capability.environment')}</dt><dd>{current.approval?.request.environment.name ?? t('ports.unreported')}</dd><dt>{t('usage.chip.agent')}</dt><dd>{current.agent}</dd><dt>{t('ports.insidePort')}</dt><dd>{current.port}</dd><dt>{t('ports.audience')}</dt><dd>{t(`ports.way.${current.way}`)}</dd></dl>
    <p className="mt-1 text-sm text-text-muted">{current.reason}</p>
    {current.address !== null && <p className="mt-2 break-all font-mono text-sm">{current.address}</p>}
    <p className="mt-2 text-sm">{current.audience}</p><p className="flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{t(`ports.transport.${current.transports}`)}</span>{current.link_carriers!==null && <span>{t(`ports.carrier.${current.link_carriers}`)}</span>}</p>
    {current.until !== null && <p className="text-sm text-text-muted">{t('ports.until')} <RelativeTime at={current.until} /></p>}
    {current.approval?.expires_at !== undefined && current.approval.expires_at !== null && state === 'pending' && <p className="mt-2 text-xs text-text-muted">{t('ports.requestPendingUntil', { time: new Date(current.approval.expires_at).toLocaleString(i18n.language) })}</p>}
    {plan !== undefined && <div className="mt-3"><PortEntries entries={current.approval?.request.opening.entries ?? []} />{plan.blockers.map((blocker) => <p key={blocker.code} className="text-sm text-danger-text">{blocker.message}</p>)}</div>}
    {plan !== undefined && <details open={programOpen} onToggle={(event) => setProgramOpen(event.currentTarget.open)} className="mt-3 rounded-md border border-border p-2"><summary className="cursor-pointer text-sm inline-flex items-center gap-2"><ActionIcon action={programOpen ? 'hide' : 'show'} />{t('ports.programTitle')}</summary><dl className="mt-2 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 break-all text-sm"><dt>{t('ports.scheme')}</dt><dd>{(plan.request.program ?? plan.target.program).http_scheme.toUpperCase()}</dd><dt>{t('ports.hostHeader')}</dt><dd>{(plan.request.program ?? plan.target.program).host_header ?? t('ports.passHost')}</dd></dl>{(plan.request.program ?? plan.target.program).proxy_protocol && <p className="mt-1 text-xs text-warning-text">{t('ports.proxyHelp')}</p>}</details>}
    {current.way === 'link' && <div className="mt-3 flex flex-col gap-2"><InlineAlert tone="warning">{forMeOnly ? t('ports.linkForMe') : t('ports.linkSharedWithAgent')}</InlineAlert><Checkbox label={t('ports.forMeOnly')} checked={forMeOnly} disabled={busy || !current.can_decide || !live || state !== 'pending'} onCheckedChange={setForMeOnly} /></div>}
    {state === 'pending' && (!current.can_decide || !live) && <p className="mt-2 text-sm text-text-muted">{t(!live ? 'approvals.waitForLine' : 'approvals.cannotAnswer')}</p>}
    {canGrant && state === 'pending' && <p className="mt-3 text-xs text-text-muted">{t(`ports.grants.cardHelp.${current.way}`)}</p>}
    {stale && state === 'pending' && <div className="mt-3"><InlineAlert tone="warning">{t('ports.staleRequest')}</InlineAlert><Button icon={<RefreshCw aria-hidden />} pending={busy} onClick={() => void refresh()}>{t('ports.reviewRequest')}</Button></div>}
    {state === 'pending' && current.can_decide && <div className="mt-3 flex flex-col gap-2 sm:flex-row sm:flex-wrap">
      <Button action="allow" variant="primary" icon={<Check aria-hidden />} pending={busy} blocked={!live || stale || (plan?.blockers.length ?? 0) > 0} onClick={() => void decide('once')}>{t('craftJoined.allowPortOnce', { port: current.port })}</Button>
      {canGrant && <Button action="allow" icon={<Repeat aria-hidden />} pending={busy} blocked={!live || stale || (plan?.blockers.length ?? 0) > 0} onClick={() => void decide('grant')}>{t('craftJoined.grantPort')}</Button>}
      <Button action="deny" variant="danger-ghost" icon={<X aria-hidden />} pending={busy} blocked={!live} onClick={() => void decide('deny')}>{t('craftJoined.denyPort', { port: current.port })}</Button>
    </div>}
    {result?.delivered_to !== null && result?.delivered_to !== undefined && <p className="mt-2 text-sm text-text-muted">{t('ports.requestDelivered', { recipient: t(`ports.recipient.${result.delivered_to}`) })}</p>}
    {result?.delivered_to === 'human' && result.link_url_once !== null && <div className="mt-3"><OnceLink delivery={{ url_once: result.link_url_once, browser_url_once: result.browser_url_once ?? null, helper_link_once: result.helper_link_once ?? null }} transports={transportsFor(current.transports)} /></div>}
    <div className="mt-3"><PortFault error={error} /></div>
  </div>;
  if (current.approval === undefined) return <section className={`min-w-0 rounded-lg bg-surface p-3 ${state === 'pending' ? 'border-2 border-warning' : 'border border-border'}`} aria-label={t('ports.requestTitle')}>{body}</section>;
  // The frame reads only safe common metadata while the generated kind catches up.
  const common = (result?.request.approval ?? current.approval) as unknown as Approval;
  return current.state !== 'pending' && result === null ? <DecidedCard approval={common}>{body}</DecidedCard> : <CardFrame approval={common} quiet={state !== 'pending'}>{body}</CardFrame>;
}

/** Mounts in the ordinary Approval list and thread item, using their live-connection gate. */
export function PortApprovalCard({ approval, live, onSettled }: { approval: PortApproval; live: boolean; onSettled?: (approval: Approval) => void }) {
 return <PortRequestCard request={requestView(approval)} live={live} onSettled={(request) => { if (request.approval !== undefined) onSettled?.(request.approval); }} />;
}
export function PortRequests({ environment, onChanged }: { environment: Environment; onChanged: () => void }) {
  const live = useSocketStatus().phase === 'ready';
  const query = useQuery({ ...portRequestsQuery(environment.id), enabled: environment.access.permissions.includes('ports.manage') });
  const [settled, setSettled] = useState<ReadonlyMap<string, PortRequest>>(new Map());
  if (!environment.access.permissions.includes('ports.manage') || query.isPending) return null;
  if (query.isError) return <PortFault error={query.error} />;
  const cards = [...query.data, ...[...settled.values()].filter((request) => !query.data.some((row) => row.id === request.id))];
  return cards.length === 0 ? null : <div className="flex flex-col gap-3">{cards.map((request) => <PortRequestCard key={request.id} request={request} live={live} onChanged={onChanged} onSettled={(safe) => setSettled((old) => new Map(old).set(safe.id, safe))} />)}</div>;
}
