/** The same live native list on a desktop dialog and a phone sheet, with reviewed exact End and actual outcomes. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { endBackground } from '@/api/background';
import { jobQuery, isOver } from '@/api/forks';
import type { BackgroundCommand, BackgroundEndRequest, BackgroundEndResult, ThreadBackground } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatPercent } from '@/lib/format';
import { newActKey } from '@/lib/requests';
import { socket } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { Drawer } from '@/ui/Drawer';
import { ErrorAlert } from '@/ui/ErrorState';
import { RelativeTime } from '@/ui/RelativeTime';
import { useRunning } from './live';
import { canEndAll, endResults, loadReview, reviewedRequest } from './review';

/** The phone boundary matches the application's md layout breakpoint. */
const PHONE = '(max-width: 767px)';
/** Selected requests have the same two-hundred-id bound as the served contract. */
const SELECTED_MAX = 200;

function CommandCard({ thread, command, canEnd, checked, onCheck, onEnd }: { thread: string; command: BackgroundCommand; canEnd: boolean; checked: boolean; onCheck: (checked: boolean) => void; onEnd: () => void }) {
  const { t } = useTranslation(); const format = useFormatContext();
  return <li className="flex flex-col gap-2 rounded-lg border border-border p-3">
    <div className="flex min-w-0 items-start justify-between gap-2">
      {canEnd ? <Checkbox checked={checked} onCheckedChange={onCheck} label={<code className="break-all text-sm">{command.command ?? t('running.unknownCommand')}</code>} /> : <code className="break-all text-sm">{command.command ?? t('running.unknownCommand')}</code>}
      {canEnd && <Button size="sm" variant="ghost" onClick={onEnd}>{t('running.end')}</Button>}
    </div>
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs text-text-muted">
      <dt>{t('running.directory')}</dt><dd className="break-all">{command.cwd_redacted ? t('running.redacted') : command.cwd ?? t('running.notReported')}</dd>
      <dt>{t('running.since')}</dt><dd>{command.since === null ? t('running.notReported') : <RelativeTime at={command.since} />}</dd>
      <dt>{t('running.place')}</dt><dd>{t(`running.place_${command.place}`)}</dd>
      {command.cpu_percent !== null && <><dt>{t('running.cpu')}</dt><dd>{formatPercent(command.cpu_percent / 100, format.locale)}</dd></>}
      {command.memory_bytes !== null && <><dt>{t('running.memory')}</dt><dd>{formatBytes(command.memory_bytes, format.locale)}</dd></>}
    </dl>
    {command.item_id !== null && <Link to="/thread/$threadId" params={{ threadId: thread }} search={{ item: command.item_id }} className="w-fit text-xs text-accent underline">{t('running.source')}</Link>}
    {command.command_truncated && <p className="text-xs text-text-subtle">{t('running.shortened')}</p>}
  </li>;
}

export function RunningDialog({ thread, close }: { thread: string; close: () => void }) {
  const { t } = useTranslation();
  const [phone, setPhone] = useState(() => window.matchMedia(PHONE).matches);
  const [roster, setRoster] = useState<ThreadBackground | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);
  const [confirm, setConfirm] = useState<{ request: BackgroundEndRequest; key: string; count: number } | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [results, setResults] = useState<BackgroundEndResult['results']>([]);
  const transfer = useRef<AbortController | null>(null);
  const refresh = useCallback(async () => {
    transfer.current?.abort(); const controller = new AbortController(); transfer.current = controller;
    setLoading(true); setRoster(null); setSelected(new Set()); setConfirm(null); setError(null);
    try { const current = await loadReview(thread, controller.signal); if (!controller.signal.aborted) setRoster(current); }
    catch (error) { if (!controller.signal.aborted) setError(error); }
    finally { if (!controller.signal.aborted) setLoading(false); }
  }, [thread]);
  useEffect(() => {
    let active = true;
    queueMicrotask(() => { if (active) void refresh(); });
    return () => { active = false; transfer.current?.abort(); };
  }, [refresh]);
  useEffect(() => { const media = window.matchMedia(PHONE); const change = () => setPhone(media.matches); media.addEventListener('change', change); return () => media.removeEventListener('change', change); }, []);
  useEffect(() => socket.status.subscribe((status, previous) => {
    if (status.phase !== 'ready') { transfer.current?.abort(); setRoster(null); setConfirm(null); setLoading(false); }
    else if (previous.phase !== 'ready') void refresh();
  }), [refresh]);
  const end = useMutation({ mutationFn: ({ request, key }: { request: BackgroundEndRequest; key: string }) => endBackground(thread, request, key), onSuccess: (job) => { setJobId(job.id); setConfirm(null); setRoster(null); } });
  const job = useQuery({ ...jobQuery(jobId ?? ''), enabled: jobId !== null });
  const finished = useRef<string | null>(null);
  useEffect(() => {
    const row = job.data;
    if (row === undefined || !isOver(row.state) || finished.current === row.id) return;
    let active = true;
    queueMicrotask(() => { if (active) { finished.current = row.id; setResults(endResults(row.result)); void refresh(); } });
    return () => { active = false; };
  }, [job.data, refresh]);
  const choose = (ids: readonly string[] | 'all') => {
    if (roster === null) return;
    const request = reviewedRequest(roster, ids);
    if (request !== null) { end.reset(); setConfirm({ request, key: newActKey(), count: ids === 'all' ? roster.total ?? 0 : ids.length }); }
  };
  const busy = end.isPending || (job.data !== undefined && !isOver(job.data.state));
  const unavailableMessage = t(roster?.state === 'offline' ? 'running.offline' : 'running.unavailable');
  const body = <div className="flex flex-col gap-3 py-2">
    {loading && <p role="status" className="text-sm text-text-muted">{t('running.loading')}</p>}
    {error !== null && <ErrorAlert error={error} />}
    {roster === null && !loading && error === null && !busy && <p role="status" className="text-sm text-text-muted">{t('running.offline')}</p>}
    {roster !== null && <>
      {roster.state !== 'live' || !roster.supported ? <p role="status" className="text-sm text-text-muted">{unavailableMessage}</p> : null}
      {roster.total === null && roster.state === 'live' && <p className="text-sm text-text-muted">{t('running.partial')}</p>}
      {roster.state === 'live' && roster.total === 0 && <EmptyState compact title={t('running.empty')} />}
      <ul className="flex flex-col gap-2">{roster.items.map((command) => <CommandCard key={command.id} thread={thread} command={command} canEnd={roster.can_end && !busy} checked={selected.has(command.id)} onCheck={(checked) => setSelected((current) => { const next = new Set(current); if (checked && next.size < SELECTED_MAX) next.add(command.id); else if (!checked) next.delete(command.id); return next; })} onEnd={() => choose([command.id])} />)}</ul>
    </>}
    {job.data !== undefined && (job.data.error === null ? <p role="status" className="text-sm text-text-muted">{t('running.job', { percent: job.data.progress.percent })}</p> : <ErrorAlert error={job.data.error} />)}
    {job.error !== null && <ErrorAlert error={job.error} />}
    {results.length > 0 && <ul aria-label={t('running.results')} className="flex flex-col gap-1 text-sm">{results.map((result) => <li key={result.id} className="flex flex-wrap gap-2"><span>{t(`running.result_${result.state}`)}</span>{result.reason !== null && <span className="text-text-muted">{result.reason}</span>}{result.item_id !== null && <Link to="/thread/$threadId" params={{ threadId: thread }} search={{ item: result.item_id }} className="text-accent underline">{t('running.source')}</Link>}</li>)}</ul>}
  </div>;
  const footer = <>
    <Button variant="ghost" onClick={() => void refresh()} disabled={busy || loading}>{t('running.refresh')}</Button>
    {roster?.can_end === true && <><Button variant="secondary" disabled={busy || selected.size === 0} onClick={() => choose([...selected])}>{t('running.endSelected', { count: selected.size })}</Button><Button disabled={busy || !canEndAll(roster)} onClick={() => choose('all')}>{t('running.endAll', { count: roster.total ?? 0 })}</Button></>}
  </>;
  return <>
    {phone ? <Drawer open onOpenChange={(open) => { if (!open) close(); }} title={t('running.title')} side="bottom"><div className="flex flex-col gap-3 p-4">{body}<div className="flex flex-wrap justify-end gap-2">{footer}</div></div></Drawer> : <Dialog open onOpenChange={(open) => { if (!open) close(); }} title={t('running.title')} size="lg" footer={footer}>{body}</Dialog>}
    {confirm !== null && <ConfirmDialog open onOpenChange={(open) => { if (!open) setConfirm(null); }} title={t('running.confirmTitle')} confirmLabel={t('running.confirm', { count: confirm.count })} pending={end.isPending} error={end.error} onConfirm={() => end.mutate(confirm)}>{t('running.confirmDetail', { count: confirm.count })}</ConfirmDialog>}
  </>;
}

export function RunningControl({ threadId }: { threadId: string }) {
  const { t } = useTranslation(); const { roster, closed } = useRunning(threadId); const [open, setOpen] = useState(false);
  if (closed || (!open && roster?.supported === true && roster.state === 'live' && roster.total === 0)) return null;
  const label = roster?.state === 'live' && roster.total !== null ? t('running.count', { count: roster.total }) : t('running.title');
  return <><Button size="sm" variant="ghost" onClick={() => setOpen(true)}>{label}</Button>{open && <RunningDialog key={threadId} thread={threadId} close={() => setOpen(false)} />}</>;
}
