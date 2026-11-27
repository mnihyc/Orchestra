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

import { EnvironmentPreparationFacts } from '@/features/thread/EnvironmentPreparationFacts';
import { threadScreenFactsQuery } from '@/api/threads';
import { readData, useReadRecovery } from '@/api/reads';
import { useViewShown } from '@/features/workbench/viewOpening';
import { ReadFeedback } from '@/ui/ReadFeedback';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { HandoffRecoveryLine, ThreadCallRecovery } from '@/features/thread/CallRecovery';
import { NowStop, ThreadNowProvider, type ThreadNow, useProvidedThreadNow } from '@/features/thread/NowLine';
import { StartWaitFacts, startWaitWords } from '@/features/thread/StartWaitFacts';
import { hasPausedService } from '@/features/timeline/rows';
import { silentAfterMs } from '@/features/timeline/silence';
import { thinkingLine } from '@/features/timeline/thinking';
import { turnWaiting } from '@/features/timeline/turnTime';
import { stoppingSince, stoppingWords, waitingSince, waitingWords } from '@/features/timeline/waiting';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { runningTurn, useThreadPart, waitingInputs } from '@/realtime/stores/thread';

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

/**
 * One clock for a start across its steps: from the earliest step this page saw of it (each step says only when it began
 * itself), else from when its message was sent. `key` names the start (the message it is for); a new start counts afresh.
 */
function useStartOrigin(key: string | null, stepSince: string | null): number | null {
  const [earliest, setEarliest] = useState<{ key: string; at: number } | null>(null);
  const step = stepSince === null ? Number.NaN : Date.parse(stepSince);
  const held = key !== null && earliest?.key === key ? earliest.at : Number.NaN;
  const at = Number.isNaN(held) ? step : Number.isNaN(step) ? held : Math.min(held, step);
  // Adjusted while rendering, as React asks for a value that follows others.
  if (key === null ? earliest !== null : !Number.isNaN(at) && (earliest?.key !== key || earliest.at !== at)) setEarliest(key === null ? null : { key, at });
  return key === null || Number.isNaN(at) ? null : at;
}

function Line({ threadId, now: current, withStop, pending, blockedReasonShown }: { threadId: string; now: ThreadNow; withStop: boolean; pending: boolean; blockedReasonShown: boolean }) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { thread, turn, stopReadOnly } = current;
  const stoppedAt = stoppingSince(thread, turn);
  const stopping = pending || stoppedAt !== null;
  const pausedService = useThreadPart(threadId, hasPausedService);
  // A thread that starts has done so since the first of its steps began, where the rows say it; else for as long as the message that asked for it has waited.
  const askedAt = useThreadPart(threadId, (view) => waitingInputs(view)[0]?.created_at);
  const startKey = useThreadPart(threadId, (view) => view.thread?.state === 'starting' ? runningTurn(view)?.input_id ?? waitingInputs(view)[0]?.id ?? 'start' : null);
  const startOrigin = useStartOrigin(startKey, waitingSince(thread));
  // Something arrived when a row came, or the text of an answer grew.
  const signal = useThreadPart(threadId, (view) => `${view.thread?.seq ?? 0}|${view.items.size}|${view.outputCounts.counts?.ascii ?? 0}|${view.outputCounts.counts?.other ?? 0}`);
  const thinking = useThreadPart(threadId, thinkingLine);
  const active = !stopReadOnly && thread !== null && (stopping || !current.over && (thread.state === 'running' || thread.state === 'starting' || thread.state === 'waiting_approval'));
  const clock = useRunningClock(active);
  const generating = !stopping && thread?.state === 'running' && turn !== null && turn.state === 'running' && !turnWaiting(turn) && turn.phase?.kind !== 'compacting';
  const starting = !stopping && thread?.state === 'starting';
  const shown = useViewShown();
  // The silence setting also says when a step of a start has gone on too long without a change.
  const needed = active && shown && (generating && !current.toolRunning || starting);
  const options = threadScreenFactsQuery(threadId);
  const facts = useQuery({ ...options, enabled: needed });
  useReadRecovery(facts, needed, options.queryKey);
  const screen = readData(facts);
  const silent = useSilentFor(signal, generating && !current.toolRunning);
  if (thread === null || stopReadOnly) return null;
  const serviceRecovery = pausedService ? <ThreadCallRecovery thread={thread} serviceOnly /> : null;
  // A model call's recovery is this line's while it is scheduled or running (its Retry now and Stop) and the banner's once it
  // needs a person: the recovery itself says which, and the line keeps no place for the banner's.
  if (!stopping && thread.blocked?.detail === 'call_recovery' && thread.lock === null) return <ThreadCallRecovery thread={thread} place={(shown) => <StatusPlace place="live" threadId={threadId} fixed={[{ id: 'call-recovery', content: shown }]} />} />;
  // Any other visible recovery owns its complete reason and acts; read-only surfaces retain their own words.
  if(!stopping&&blockedReasonShown&&thread.state==='blocked'&&thread.blocked!==null&&thread.lock===null)return serviceRecovery;

  const words = waitingWords(thread, t);
  // A step of a start that has not moved on for the silence setting says so beside its clock: a start that hangs is
  // told apart from one that is slow. A step with a start deadline of its own says that deadline instead.
  const stepSince = waitingSince(thread);
  const stalledFor = starting && stepSince !== null ? Math.max(0, clock - Date.parse(stepSince)) : 0;
  let line: string | null = null;
  if (stopping) line = stoppingWords(thread, t, turn) ?? t('thread.stopping.line');
  else if (thread.lock !== null) line = words;
  else if (thread.state === 'waiting_approval') line = t('thread.now.needsInput');
  else if (thread.state === 'starting') {
    const deadline = startWaitWords(thread, turn, t, format);
    const step = words ?? t('thread.now.starting');
    line = deadline ?? (stalledFor >= silentAfterMs(screen?.silent_after_secs) ? t('timeline.status.startSilent', { step }) : step);
  }
  else if (thread.state === 'blocked') line = words;
  // A turn the page still holds as running does not make a thread that says it is idle "Generating".
  else if (thread.state === 'running' && !current.over) {
    line = silent >= silentAfterMs(screen?.silent_after_secs) ? t('timeline.status.silent', { name: thread.binding?.agent.name ?? thread.name, time: formatSpan(silent, format) }) : (thinking ?? current.text) || t('thread.now.working');
  }
  if (line === null) return serviceRecovery;
  // Until the turn runs there is no elapsed time of its own to show: one clock for the whole start stands in its place,
  // through every step and once the turn row exists, so that no step of a start is without a clock and none starts it again.
  const waited = stoppedAt !== null ? formatSpan(Math.max(0, clock - Date.parse(stoppedAt)), format)
    : thread.state === 'starting' ? formatSpan(Math.max(0, clock - (startOrigin ?? Date.parse(stepSince ?? askedAt ?? thread.updated_at))), format) : null;
  const status = (
    <div data-status-line className="flex min-w-0 items-center gap-2 py-1 text-sm text-text-muted">
      <StatusPlace place="live" threadId={threadId} activeLabel={line} role="status" fixed={[
        ...(thread.state === 'blocked' ? [] : [{ id: 'working-mark', content: <LoaderCircle className="animate-spin motion-reduce:animate-none" aria-hidden /> }]),
        ...(generating ? [] : [{ id: 'state', text: line, content: !stopping && thread.state === 'starting' ? <StartWaitFacts thread={thread} turn={turn} fallback={line} /> : <span className="min-w-0 truncate">{line}</span> }]),
        ...(thread.state === 'starting' && thread.starting_step === 'environment' ? [{ id: 'environment-preparation', content: <EnvironmentPreparationFacts stage={thread.environment_preparation} /> }] : []),
        ...(waited === null ? [] : [{ id: 'waited', text: waited, content: <span className="shrink-0 tabular-nums">{waited}</span> }]),
      ]} />
      {active && withStop && <NowStop />}
    </div>
  );
  // The activity already owns this line's loading mark and live region while its silence policy is read.
  return <>{thread.lock?.phase === 'history' ? <HandoffRecoveryLine job={thread.lock.id} fallback={status} /> : status}{needed && <ReadFeedback read={facts} />}{serviceRecovery}</>;
}

/** `withStop`: the line carries the Stop of the turn; the screen asks for it only where the composer, whose Stop it is while a turn runs, is not drawn. */
export function StatusLine({ threadId, withStop = false,blockedReasonShown=false }: { threadId: string; withStop?: boolean;blockedReasonShown?:boolean }) {
  const context = useProvidedThreadNow();
  // Where the screen has not given the activity, the line reads it itself.
  if (context === null) return <ThreadNowProvider threadId={threadId}><StatusLine threadId={threadId} withStop={withStop} blockedReasonShown={blockedReasonShown}/></ThreadNowProvider>;
  return <Line threadId={threadId} now={context.now} withStop={withStop} pending={context.stop.pending} blockedReasonShown={blockedReasonShown} />;
}
