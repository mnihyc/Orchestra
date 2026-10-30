/** A place administrator reviews all wider port admissions below it and closes through current authority. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { ExternalLink, RefreshCw, X } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { closePort, outsideOpeningsQuery } from '@/api/ports';
import { Button } from '@/ui/Button';
import { Dialog } from '@/ui/Dialog';
import { EmptyState } from '@/ui/EmptyState';
import { Pagination } from '@/ui/Pagination';
import { RelativeTime } from '@/ui/RelativeTime';
import { SkeletonRows } from '@/ui/Skeleton';
import { PortEntries } from './Entries';
import { PortFault } from './Fault';
import { openingView } from './wire';
import type { PortOpening } from '@/generated/api-types';

function OutsideRow({ opening }: { opening: PortOpening }) {
  const { t } = useTranslation(); const [confirm, setConfirm] = useState(false); const [busy, setBusy] = useState(false); const [error, setError] = useState<unknown>(null);
  const state = opening.state === 'not_listening' ? 'nothing_listens' : opening.state;
  const close = async () => { if (busy) return; setBusy(true); setError(null); try { await closePort(openingView(opening)); setConfirm(false); } catch (failed) { setError(failed); } finally { setBusy(false); } };
  return <article className="min-w-0 rounded-lg border border-border bg-surface p-3" data-testid="outside-opening">
    <div className="flex flex-wrap items-start justify-between gap-2"><div className="min-w-0"><Link to="/resources/environments/$id" params={{ id: opening.environment_id }} search={{ tab: 'ports', port: opening.port }} className="break-all text-sm font-semibold text-accent-text hover:underline">{opening.entries.find((entry) => entry.address !== null)?.address ?? t('ports.portNumber', { port: opening.port })}</Link><p className="text-xs text-text-muted">{t('ports.portNumber', { port: opening.port })}</p></div><Button size="sm" variant="danger-ghost" icon={<X aria-hidden />} onClick={() => setConfirm(true)}>{t('ports.closeOpening')}</Button></div>
    <p className="mt-2 break-words text-sm">{opening.kind === 'place' ? t('ports.outsidePlace', { place: opening.place?.name ?? t('ports.hiddenPlace') }) : opening.kind === 'link' ? t('ports.linkCaller', { label: opening.label }) : t('ports.way.host')}</p>
    <span className="mt-1 inline-block rounded-full border border-border px-2 py-1 text-xs">{t(`ports.state.${state}`)}</span>
    <p className="mt-2 flex flex-wrap gap-x-3 gap-y-1 text-xs text-text-muted"><span>{t('ports.askedBy', { name: opening.requested_by_thread?.name ?? opening.requested_by?.display_name ?? t('ports.removedRequester') })}</span><span>{t('ports.allowedBy', { name: opening.allowed_by?.display_name ?? t('ports.unreported') })}</span></p>
    {opening.until !== null && <p className="text-xs text-text-muted">{t('ports.until')} <RelativeTime at={opening.until} /></p>}
    {opening.reason !== null && <p className="mt-1 text-xs text-warning-text">{opening.reason}</p>}
    <div className="mt-3"><PortEntries entries={opening.entries} /></div>
    {!opening.host_allocated && opening.kind === 'host' && <p className="mt-1 text-xs text-text-muted">{t('ports.liveOnly')}</p>}
    <Dialog open={confirm} onOpenChange={setConfirm} title={t('ports.closeTitle')} description={t('ports.closeAbout')} dismissible={!busy} footer={<><Button icon={<X aria-hidden />} onClick={() => setConfirm(false)}>{t('ports.cancel')}</Button><Button variant="danger" icon={<X aria-hidden />} pending={busy} onClick={() => void close()}>{t('ports.closeOpening')}</Button></>}><p className="flex flex-wrap gap-x-3 gap-y-1 break-all text-sm"><span>{t('ports.portNumber', { port: opening.port })}</span><span>{opening.label === '' ? opening.place?.name ?? opening.host?.name : opening.label}</span></p>{!opening.can.close && <p className="mt-2 text-sm text-text-muted">{t('ports.closeNeedsManage')}</p>}<PortFault error={error} /></Dialog>
  </article>;
}
export function OutsideOpenings({ placeId }: { placeId: string }) {
  const { t } = useTranslation(); const query = useInfiniteQuery(outsideOpeningsQuery(placeId));
  const items = query.data?.pages.flatMap((page) => page.items) ?? [];
  return <section className="flex min-w-0 flex-col gap-3" aria-label={t('ports.outsideTitle')} data-testid="outside-ports"><header className="flex flex-wrap items-center justify-between gap-2"><h2 className="text-lg font-semibold">{t('ports.outsideTitle')}</h2><Button size="sm" icon={<RefreshCw aria-hidden />} pending={query.isFetching} onClick={() => void query.refetch()}>{t('ports.refresh')}</Button></header><p className="text-sm text-text-muted">{t('ports.outsideAbout')}</p>
    {query.isPending ? <SkeletonRows rows={3} /> : query.isError && items.length === 0 ? <PortFault error={query.error} onRefresh={() => void query.refetch()} /> : <>{items.length === 0 ? <EmptyState compact icon={<ExternalLink aria-hidden />} title={t('ports.outsideEmpty')} /> : items.map((opening) => <OutsideRow key={opening.id} opening={opening} />)}<Pagination hasMore={query.hasNextPage} loading={query.isFetchingNextPage} error={query.isFetchNextPageError ? query.error : null} onMore={() => void query.fetchNextPage()} /></>}
  </section>;
}
