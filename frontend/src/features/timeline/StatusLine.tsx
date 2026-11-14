/** What the thread is doing now, as the one line of the dock: a spinner, what is being done and one clock.
 *
 * One line, for as long as a turn runs or the thread waits, and it stays on the screen with the composer. It names
 * what is being done now (the running step's own title, "Thinking…", "Writing the answer", the step of a start), says
 * "Needs your input" while an approval waits, and, when nothing has arrived for as long as the thread's silence notice
 * says (two minutes unless it is set otherwise) while no tool runs, says how long nothing came. Its clock is the turn's elapsed time and the only one: the words never carry another. It is the
 * one live region of the page, and it never scrolls the conversation by itself. The Stop of a running turn is the composer's;
 * the line carries one only where the composer is not drawn (a selection being made, a sub-agent's conversation).
 */
import { useQuery } from '@tanstack/react-query';
import { LoaderCircle } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { settingsQuery } from '@/api/settings';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { HandoffRecoveryLine, ThreadCallRecovery } from '@/features/thread/CallRecovery';
import { NowStop, ThreadNowProvider, type ThreadNow, useProvidedThreadNow } from '@/features/thread/NowLine';
import { hasPausedService } from '@/features/timeline/rows';
import { silentAfterMs } from '@/features/timeline/silence';
import { thinkingLine } from '@/features/timeline/thinking';
import { turnWaiting } from '@/features/timeline/turnTime';
import { waitingSince, waitingWords } from '@/features/timeline/waiting';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { useThreadPart, waitingInputs } from '@/realtime/stores/thread';

/** How long nothing has changed in the thread while `running`; counted afresh whenever something does. */
function useSilentFor(signal: string, running: boolean): number {
  const [measured, setMeasured] = useState({ signal, ms: 0 });
  useEffect(() => {
    if (!running) return;
    const since = Date.now();
    const timer = setInterval(() => setMeasured({ signal, ms: Date.now() - since }), 1_000);
    return () => clearInterval(timer);
  }, [signal, running]);
  return running && measured.signal === signal ? measured.ms : 0;
}

function Line({ threadId, now: current, withStop }: { threadId: string; now: ThreadNow; withStop: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { thread, turn, readOnly } = current;
  const pausedService = useThreadPart(threadId, hasPausedService);
  // A thread that starts has done so for as long as its step has gone on, where the row says when the step began; else for as long as the message that asked for it has waited. No turn exists yet.
  const askedAt = useThreadPart(threadId, (view) => waitingInputs(view)[0]?.created_at);
  // Something arrived when a row came, or the text of an answer grew.
  const signal = useThreadPart(threadId, (view) => `${view.thread?.seq ?? 0}|${view.items.size}|${view.outputCounts.counts?.ascii ?? 0}|${view.outputCounts.counts?.other ?? 0}`);
  const thinking = useThreadPart(threadId, thinkingLine);
  const active = !readOnly && thread !== null && (thread.state === 'running' || thread.state === 'starting' || thread.state === 'waiting_approval');
  const clock = useRunningClock(active);
  // How long nothing may come before the line says so is the thread's own setting; it is read only while the thread is at work.
  const settings = useQuery({ ...settingsQuery(threadId), enabled: active });
  const generating = thread?.state === 'running' && turn !== null && turn.state === 'running' && !turnWaiting(turn) && turn.phase?.kind !== 'compacting';
  const silent = useSilentFor(signal, generating && !current.toolRunning);
  if (thread === null || readOnly) return null;
  if (thread.blocked?.detail === 'call_recovery' && thread.lock === null) return <StatusPlace place="live" threadId={threadId} fixed={[{ id: 'call-recovery', content: <ThreadCallRecovery thread={thread} /> }]} />;

  const words = waitingWords(thread, t);
  let line: string | null = null;
  if (thread.lock !== null) line = words;
  else if (thread.state === 'waiting_approval') line = t('thread.now.needsInput');
  else if (thread.state === 'starting') line = words ?? t('thread.now.starting');
  // The environment recovery banner owns this reason and its actions above the composer.
  else if (thread.state === 'blocked') line = thread.blocked?.reason === 'environment_unavailable' ? null : words;
  // A turn the page still holds as running does not make a thread that says it is idle "Generating".
  else if (turn !== null && thread.state === 'running') {
    line = silent >= silentAfterMs(settings.data) ? t('timeline.status.silent', { name: thread.binding?.agent.name ?? thread.name, time: formatSpan(silent, format) }) : thinking ?? current.text;
  }
  const serviceRecovery = pausedService ? <ThreadCallRecovery thread={thread} serviceOnly /> : null;
  if (line === null) return serviceRecovery;
  // With no turn yet there is no elapsed time to show: the wait of the message stands in its place.
  const waited = turn === null && thread.state === 'starting' ? formatSpan(Math.max(0, clock - Date.parse(waitingSince(thread) ?? askedAt ?? thread.updated_at)), format) : null;
  const status = (
    <div data-status-line className="flex min-w-0 items-center gap-2 py-1 text-sm text-text-muted">
      <StatusPlace place="live" threadId={threadId} activeLabel={line} role="status" fixed={[
        ...(thread.state === 'blocked' ? [] : [{ id: 'working-mark', content: <LoaderCircle className="animate-spin motion-reduce:animate-none" aria-hidden /> }]),
        ...(generating ? [] : [{ id: 'state', text: line, content: <span className="min-w-0 truncate">{line}</span> }]),
        ...(waited === null ? [] : [{ id: 'waited', text: waited, content: <span className="shrink-0 tabular-nums">{waited}</span> }]),
      ]} />
      {active && withStop && <NowStop />}
    </div>
  );
  return <>{thread.lock?.phase === 'history' ? <HandoffRecoveryLine job={thread.lock.id} fallback={status} /> : status}{serviceRecovery}</>;
}

/** `withStop`: the line carries the Stop of the turn; the screen asks for it only where the composer, whose Stop it is while a turn runs, is not drawn. */
export function StatusLine({ threadId, withStop = false }: { threadId: string; withStop?: boolean }) {
  const context = useProvidedThreadNow();
  // Where the screen has not given the activity, the line reads it itself.
  if (context === null) return <ThreadNowProvider threadId={threadId}><StatusLine threadId={threadId} withStop={withStop} /></ThreadNowProvider>;
  return <Line threadId={threadId} now={context.now} withStop={withStop} />;
}
