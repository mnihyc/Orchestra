/** The server's one thread presentation, shared by icons, tabs, rows and state chips. */
import { forwardRef, useMemo } from 'react';
import type { LucideIcon, LucideProps } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';

import type { ThreadDisplay, ThreadDisplayState } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { formatRelative, formatSpan, useNow, useRunningClock, type TimeContext } from '@/lib/time';
import { useTreeNode, useTreeStore } from '@/realtime/stores/tree';
import { useThreadPart } from '@/realtime/stores/thread';
import { containedThreadStates, currentThreadDisplay } from '@/lib/threadDisplay';
export { THREAD_URGENCY, isThreadUrgency } from '@/lib/threadDisplay';
import { NodeIcon } from '@/ui/NodeIcon';
import { StateChip, StateIcon, type StateIconProps } from '@/ui/StateChip';

/** The display state is supplied by the server; missing metadata never infers private work. */
export const threadDisplayState = (display: ThreadDisplay | null | undefined): ThreadDisplayState => display?.state ?? 'idle';

// Stored guard windows use seconds; the shared duration formatter takes milliseconds.
const SECOND_MS = 1_000;

/** A short localized fragment with only the safe particulars the server supplied. */
export function threadStateLabel(display: ThreadDisplay, t: TFunction, context: TimeContext, now: number): string {
  const recovery = display.recovery;
  if (recovery !== undefined && recovery !== null) {
    const kind = t(`calls.kinds.${recovery.failure_kind}`);
    if (display.state === 'failed') {
      if (recovery.stop_reason === 'repeat_limit') return t('calls.repeatStopped', {
        kind, count: recovery.count ?? 0,
        occurrences: recovery.count === null ? '–' : formatNumber(recovery.count, context.locale),
        window: recovery.window_secs === null ? '–' : formatSpan(recovery.window_secs * SECOND_MS, context, 'short'),
      });
      return recovery.stop_reason === null ? kind : t(`calls.details.${recovery.stop_reason}`);
    }
    if (display.state === 'queued') {
      const phrase = recovery.due_at === null ? t('state.queued') : t(
        recovery.step === 'wait' ? 'threadStates.waitIn' : recovery.step === 'failover' ? 'threadStates.failoverIn' : recovery.continuing ? 'threadStates.continueIn' : 'threadStates.retryIn',
        { time: formatSpan(Math.max(0, Date.parse(recovery.due_at) - now), context, 'short') },
      );
      return recovery.attempts === null ? phrase : `${phrase} · ${t('calls.attempt', {
        attempt: formatNumber(recovery.attempt, context.locale), total: formatNumber(recovery.attempts, context.locale),
      })}`;
    }
  }
  if (display.state === 'queued') return t('threadStates.queued', { count: display.queued_inputs, figure: formatNumber(display.queued_inputs, context.locale) });
  if (display.state === 'held') return t('threadStates.held');
  if (display.state === 'waiting_approval' && display.waiting_inputs > 0) return t('threadStates.waitingInputs', { count: display.waiting_inputs, figure: formatNumber(display.waiting_inputs, context.locale) });
  if (display.state === 'waiting_approval' && display.waiting_for_answer) return t('thread.header.waitingForAnswer');
  if (display.state === 'blocked' && display.blocked_reason === 'unbound') return t('thread.header.needsSetup');
  if (display.state === 'blocked' && display.blocked_reason !== null) return t('threadStates.blocked', { reason: t(`thread.blockedWhy.${display.blocked_reason}`) });
  if (display.state === 'finished' && display.finished_at !== null) return t('threadStates.finished', { time: formatRelative(display.finished_at, context, now) });
  return t(`state.${display.state}`);
}

export function useThreadStateLabel(display: ThreadDisplay | null | undefined): string | undefined {
  const { t } = useTranslation();
  const context = useFormatContext();
  const now = useNow();
  const recovery = display?.recovery;
  const countdown = display?.state === 'queued' && recovery !== undefined && recovery !== null && recovery.due_at !== null;
  const seconds = useRunningClock(countdown);
  return display === null || display === undefined ? undefined : threadStateLabel(display, t, context, countdown ? seconds : now);
}

/** The small state record carried with a thread's existing heading and tab name. */
export interface ThreadIconFacts { id: string; display: ThreadDisplay }

/** A cached row keeps its identity until its state facts arrive. */
export function threadIconFacts(thread: { id: string; display?: ThreadDisplay | null | undefined } | null | undefined): ThreadIconFacts | undefined {
  if (!thread?.display) return undefined;
  return { id: thread.id, display: thread.display };
}

type Props = Omit<StateIconProps, 'state'> & { display: ThreadDisplay | null | undefined };

/** No request or feed is acquired by a thread icon. A reader without metadata keeps the plain glyph. */
export function ThreadStateIcon({ display, label, className, glyphRef }: Props) {
  const words = useThreadStateLabel(display);
  if (display === null || display === undefined) return <NodeIcon kind="thread" {...(className === undefined ? {} : { className })} />;
  return <StateIcon state={display.state} label={label ?? words} className={className} glyphRef={glyphRef} />;
}

/** The same presentation with its readable words where there is room. */
export function ThreadStateChip({ display, className }: Pick<Props, 'display' | 'className'>) {
  const label = useThreadStateLabel(display);
  if (display === null || display === undefined) return <ThreadStateIcon display={display} className={className} />;
  return <StateChip state={threadDisplayState(display)} {...(label === undefined ? {} : { label })} {...(className === undefined ? {} : { className })} />;
}

/** A visible tab reads already held live facts even while its content reader is paused. */
export function LiveThreadStateIcon({ display, threadId, className, glyphRef }: Props & { threadId?: string | undefined }) {
  const row = useTreeNode(threadId);
  const cached = useThreadPart(threadId ?? '', (view) => view.closed === null ? view.thread?.display : null);
  const latest = cached === null ? undefined : currentThreadDisplay(cached, currentThreadDisplay(row?.thread?.display, display ?? undefined));
  return <ThreadStateIcon display={latest} className={className} glyphRef={glyphRef} />;
}

/** Cached, unopened tabs already have a tree state and need no reader of their own. */
export const THREAD_GLYPHS = Object.fromEntries(['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished', 'idle', 'archived'].map((state) => [state, forwardRef<SVGSVGElement, LucideProps>(function ThreadGlyph(props, ref) {
  return <StateIcon state={state as ThreadDisplayState} className={props.className} glyphRef={ref} />;
})])) as Record<ThreadDisplayState, LucideIcon>;

/** A place's end mark uses only authorized facts already held by its existing tree feed. */
export function ContainedThreadStateIcon({ id }: { id: string }) {
  const { t } = useTranslation();
  const trees = useTreeStore((state) => state.byOrg);
  const state = useMemo(() => {
    for (const tree of trees.values()) {
      const found = containedThreadStates(tree.nodes).get(id);
      if (found !== undefined) return found;
    }
    return null;
  }, [trees, id]);
  return state === null ? null : <StateIcon state={state} label={t(`navigation.holds.${state}`)} />;
}
