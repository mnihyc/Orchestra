/** The same live native list on a desktop dialog and a phone sheet, with reviewed exact End and actual outcomes. */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Link } from '@tanstack/react-router';
import { revealView } from '@/features/workbench/openView';
import { SquareTerminal } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';
import { endBackground } from '@/api/background';
import { jobQuery, isOver } from '@/api/forks';
import type { BackgroundCommand, BackgroundEndRequest, BackgroundEndResult, BackgroundWork, ThreadBackground } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatBytes, formatNumber, formatPercent } from '@/lib/format';
import { newActKey } from '@/lib/requests';
import { socket } from '@/realtime/live';
import { Button } from '@/ui/Button';
import { Checkbox } from '@/ui/Checkbox';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { Dialog } from '@/ui/Dialog';
import { Drawer } from '@/ui/Drawer';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { RelativeTime } from '@/ui/RelativeTime';
import { Tooltip } from '@/ui/Tooltip';
import { canEndAll, endReasonInReview, endResults, loadReview, reviewedRequest, reviewedTargetRequest, type BackgroundEndTarget } from './review';
import { backgroundName } from './backgroundWake';

/** The phone boundary matches the application's md layout breakpoint. */
const PHONE = '(max-width: 767px)';
/** Selected requests have the same two-hundred-id bound as the served contract. */
const SELECTED_MAX = 200;

function CommandCard({ thread, command, task, endReason, checked, onCheck, onEnd }: { thread: string; command: BackgroundCommand; task?: BackgroundWork; endReason: string | null; checked: boolean; onCheck: (checked: boolean) => void; onEnd: () => void }) {
  const { t } = useTranslation(); const format = useFormatContext();
  const name = task === undefined ? command.command ?? t('running.unknownCommand') : backgroundName(task, t);
  const label = task?.kind === 'subagent' ? <span className="wrap-anywhere text-sm">{name}</span> : <code className="break-all text-sm">{name}</code>;
  return <li className="flex flex-col gap-2 rounded-lg border border-border p-3">
    <div className="flex min-w-0 items-start justify-between gap-2">
      {endReason === null ? <Checkbox checked={checked} onCheckedChange={onCheck} label={label} /> : label}
      <Button size="sm" variant="ghost" blocked={endReason !== null} reason={endReason ?? undefined} onClick={onEnd}>{t('running.end')}</Button>
    </div>
    {task !== undefined && <span className="text-xs text-text-muted">{t(`backgroundWake.states.${task.state}`)}</span>}
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

function RunningWork({ thread, close, endTarget, panel = false }: { thread: string; close: () => void; endTarget?: BackgroundEndTarget; panel?: boolean }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const [phone, setPhone] = useState(() => window.matchMedia(PHONE).matches);
  const [roster, setRoster] = useState<ThreadBackground | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<unknown>(null);
  const [confirm, setConfirm] = useState<{ request: BackgroundEndRequest; key: string; count: number } | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);
  const [results, setResults] = useState<BackgroundEndResult['results']>([]);
  const transfer = useRef<AbortController | null>(null);
  const requestedEnd = useRef(false);
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
  useEffect(() => {
    if (requestedEnd.current || endTarget === undefined || roster === null || loading) return;
    let active = true;
    queueMicrotask(() => {
      if (!active || socket.status.getState().phase !== 'ready') return;
      requestedEnd.current = true;
      const request = reviewedTargetRequest(roster, endTarget);
      if (request === null) setError(new Error(t('backgroundWake.reviewChanged')));
      else setConfirm({ request, key: newActKey(), count: 1 });
    });
    return () => { active = false; };
  }, [endTarget, roster, loading, t]);
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
  const nativeEndReason = roster === null ? null : endReasonInReview(roster);
  const endReason = busy ? t('running.endBusy') : loading ? t('running.endChecking') : error !== null ? t('running.endReadFailed')
    : roster === null ? t('running.endReasons.unavailable') : nativeEndReason === null ? null : t(`running.endReasons.${nativeEndReason}`);
  const selectedEndReason = endReason ?? (selected.size === 0 ? t('running.endSelection')
    : roster === null || reviewedRequest(roster, [...selected]) === null ? t('running.endReasons.no_current_target') : null);
  const blockedCommand = roster === null ? undefined : roster.items.map((command) => endReasonInReview(roster, command)).find((reason) => reason !== null);
  const allEndReason = endReason ?? (roster !== null && canEndAll(roster) ? null : blockedCommand != null ? t(`running.endReasons.${blockedCommand}`)
    : t('running.endIncomplete'));
  const unavailableMessage = roster?.reason ? t('running.unavailableWithReason', { reason: roster.reason }) : t(roster?.state === 'offline' ? 'running.offline' : 'running.unavailable');
  const body = <div className="flex flex-col gap-3 py-2">
    {loading && <p role="status" className="text-sm text-text-muted">{t('running.loading')}</p>}
    {error !== null && <ErrorAlert error={error} />}
    {roster === null && !loading && error === null && !busy && <p role="status" className="text-sm text-text-muted">{t('running.offline')}</p>}
    {roster !== null && <>
      {roster.state !== 'live' || !roster.supported ? <p role="status" className="text-sm text-text-muted">{unavailableMessage}</p> : null}
      {roster.total === null && roster.state === 'live' && <p className="text-sm text-text-muted">{t('running.partial')}</p>}
      {roster.state === 'live' && roster.total === 0 && <EmptyState compact title={t('running.empty')} />}
      <ul className="flex flex-col gap-2">{roster.items.map((command) => {
        const task = roster.wake?.tasks.find((task) => task.end_id === command.id);
        const reason = endReasonInReview(roster, command);
        return <CommandCard key={command.id} thread={thread} command={command} {...(task === undefined ? {} : { task })} endReason={endReason ?? (reason === null ? null : t(`running.endReasons.${reason}`))} checked={selected.has(command.id)} onCheck={(checked) => setSelected((current) => { const next = new Set(current); if (checked && next.size < SELECTED_MAX) next.add(command.id); else if (!checked) next.delete(command.id); return next; })} onEnd={() => choose([command.id])} />;
      })}</ul>
    </>}
    {job.data !== undefined && (job.data.error === null ? <p role="status" className="text-sm text-text-muted">{t('running.job', { percent: job.data.progress.percent })}</p> : <ErrorAlert error={job.data.error} />)}
    {job.error !== null && <ErrorAlert error={job.error} />}
    {results.length > 0 && <ul aria-label={t('running.results')} className="flex flex-col gap-1 text-sm">{results.map((result) => <li key={result.id} className="flex flex-wrap gap-2"><span>{t(`running.result_${result.state}`)}</span>{result.reason !== null && <span className="text-text-muted">{result.reason}</span>}{result.item_id !== null && <Link to="/thread/$threadId" params={{ threadId: thread }} search={{ item: result.item_id }} className="text-accent underline">{t('running.source')}</Link>}</li>)}</ul>}
  </div>;
  const footer = <>
    {!panel && <Button variant="ghost" disabled={busy || confirm !== null} onClick={() => { revealView('background-work', { thread }); close(); }}>{t('running.openPanel')}</Button>}
    <Button variant="ghost" onClick={() => void refresh()} disabled={busy || loading}>{t('running.refresh')}</Button>
    <Button variant="secondary" blocked={selectedEndReason !== null} reason={selectedEndReason ?? undefined} onClick={() => choose([...selected])}>{t('running.endSelected', { count: selected.size, replace: { count: formatNumber(selected.size, locale) } })}</Button>
    <Button blocked={allEndReason !== null} reason={allEndReason ?? undefined} onClick={() => choose('all')}>{roster?.total == null ? t('running.endAllUnknown') : t('running.endAll', { count: roster.total, replace: { count: formatNumber(roster.total, locale) } })}</Button>
  </>;
  return <>
    {panel ? <section role="region" aria-label={t('running.title')} data-background-work-panel={thread} className="flex h-full min-h-0 flex-col gap-3 p-3"><div className="min-h-0 flex-1 overflow-auto">{body}</div><div className="flex flex-wrap justify-end gap-2 border-t border-border pt-3">{footer}</div></section> : phone ? <Drawer open onOpenChange={(open) => { if (!open) close(); }} title={t('running.title')} side="bottom"><div className="flex flex-col gap-3 p-4">{body}<div className="flex flex-wrap justify-end gap-2">{footer}</div></div></Drawer> : <Dialog open onOpenChange={(open) => { if (!open) close(); }} title={t('running.title')} size="lg" footer={footer}>{body}</Dialog>}
    {confirm !== null && <ConfirmDialog open onOpenChange={(open) => { if (!open) setConfirm(null); }} title={t('running.confirmTitle')} confirmLabel={t('running.confirm', { count: confirm.count })} pending={end.isPending} error={end.error} onConfirm={() => end.mutate(confirm)}>{t('running.confirmDetail', { count: confirm.count })}</ConfirmDialog>}
  </>;
}

/** Both presentations share one native review, immutable End identities and outcome handling. */
export function RunningDialog(props: { thread: string; close: () => void; endTarget?: BackgroundEndTarget }) {
  return <RunningWork {...props} />;
}
export function RunningWorkPanel({ thread }: { thread: string }) {
  return <RunningWork thread={thread} close={() => undefined} panel />;
}

export function RunningControl({ roster, closed, opened, onOpen }: { roster: ThreadBackground | null; closed: boolean; opened: boolean; onOpen: () => void }) {
  const { t } = useTranslation();
  const { locale } = useFormatContext();
  const total = roster?.supported === true && roster.state === 'live' ? roster.total : null;
  if (closed || (!opened && total === 0)) return null;
  const label = total === null ? t('running.title') : t('running.count', { count: total, replace: { count: formatNumber(total, locale) } });
  const firstLine = { marginBlockStart: 'calc((var(--text-lg--line-height) - var(--control-sm)) / 2)' };
  return total === null ? <IconButton size="sm" style={firstLine} label={t('running.title')} icon={<SquareTerminal aria-hidden />} aria-haspopup="dialog" aria-expanded={opened} onClick={onOpen} />
    : <Tooltip content={t('running.title')}><Button size="sm" variant="ghost" style={firstLine} aria-haspopup="dialog" aria-expanded={opened} onClick={onOpen}>{label}</Button></Tooltip>;
}
