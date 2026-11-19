/** What the thread is doing now, as one line: the phase of its turn, a job that holds it, or what it waits for.
 *
 * What the thread is doing now, when that is no item: one line in the manner of a history line, with a
 * running time, at the place where the next thing will appear. It runs while a turn runs (and names the
 * step in hand), while the harness shortens its conversation, while the thread starts, while it waits for
 * an answer to a request or for a limit or a place, and while a switch, a move or an update holds it. It
 * never scrolls the conversation by itself, and it goes when what it says is over.
 */
import { LoaderCircle } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { useTimelineContext } from '@/features/timeline/context';
import { describeStep } from '@/features/timeline/titles';
import { waitingWords } from '@/features/timeline/waiting';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { runningTurn, useThreadPart, waitingInputs } from '@/realtime/stores/thread';
import { HandoffRecoveryLine, ThreadCallRecovery } from '@/features/thread/CallRecovery';
import { StatusPlace } from '@/features/status-items/StatusPlace';
import { turnWaiting } from '@/features/timeline/turnTime';

export function StatusLine() {
  const { t } = useTranslation();
  const format = useFormatContext();
  const { threadId, readOnly } = useTimelineContext();
  const thread = useThreadPart(threadId, (view) => view.thread);
  const turn = useThreadPart(threadId, runningTurn);
  const step = useThreadPart(threadId, (view) => {
    for (let at = view.order.length - 1; at >= 0 && at > view.order.length - 40; at -= 1) {
      const item = view.items.get(view.order[at] ?? '');
      if (item?.kind === 'tool_call' && item.status === 'in_progress') return item;
    }
    return undefined;
  });
  const asked = useThreadPart(threadId, (view) => {
    for (let at = view.order.length - 1; at >= 0 && at > view.order.length - 60; at -= 1) {
      const item = view.items.get(view.order[at] ?? '');
      if (item?.kind === 'approval' && item.status === 'pending') return item.created_at;
    }
    return undefined;
  });
  // A thread that starts has done so for as long as the turn it starts, or the message that asked for it, has waited.
  const askedAt = useThreadPart(threadId, (view) => waitingInputs(view)[0]?.created_at);
  const now = useRunningClock(readOnly !== true && thread !== null && (thread.state === 'running' || thread.state === 'starting' || thread.state === 'waiting_approval'));
  if (thread === null || readOnly === true) return null;
  if (thread.blocked?.detail === 'call_recovery' && thread.lock === null) return <StatusPlace place="live" threadId={threadId} fixed={[{ id: 'call-recovery', content: <ThreadCallRecovery thread={thread} /> }]} />;

  let line: string | null = null;
  const since = (at: string | null | undefined): string => formatSpan(Math.max(0, now - Date.parse(at ?? new Date(now).toISOString())), format);
  const words = waitingWords(thread, t);
  if (thread.lock !== null) line = words;
  else if (thread.state === 'waiting_approval') line = t('timeline.status.waitingApproval', { time: since(turn?.waiting_since ?? asked ?? turn?.started_at) });
  else if (thread.state === 'starting') line = words ?? t('timeline.status.starting', { name: thread.binding?.agent.name ?? thread.name, time: since(turn?.started_at ?? askedAt ?? thread.updated_at) });
  // The environment recovery banner owns this reason and its actions above the composer.
  else if (thread.state === 'blocked') line = thread.blocked?.reason === 'environment_unavailable' ? null : words;
  else if (turn !== null) {
    line =
      turn.phase?.kind === 'compacting'
        ? t('timeline.status.compacting', { time: since(turn.phase.since) })
        : `${t('statusItems.generating')}${step === undefined ? '' : `: ${describeStep(step, t).text}`}`;
  }
  if (line === null) return null;
  const status = (
    <div data-status-line className="flex min-w-0 items-center justify-center gap-2 py-1 text-center text-sm text-text-muted">
      <StatusPlace place="live" threadId={threadId} activeLabel={line} role="status" fixed={[
        ...(thread.state === 'blocked' ? [] : [{ id: 'working-mark', content: <LoaderCircle className="animate-spin motion-reduce:animate-none" aria-hidden /> }]),
        ...(thread.state === 'running' && turn?.state === 'running' && !turnWaiting(turn) && turn.phase?.kind !== 'compacting' ? [] : [{ id: 'state', text: line, content: <span className="min-w-0 truncate">{line}</span> }]),
      ]} />
    </div>
  );
  return thread.lock?.phase === 'history' ? <HandoffRecoveryLine job={thread.lock.id} fallback={status} /> : status;
}
