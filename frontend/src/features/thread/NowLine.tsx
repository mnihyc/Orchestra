/** Current work is read from the same admitted turn and items as the conversation. */
import type { TFunction } from 'i18next';
import { createContext, useContext, useMemo, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useQueries } from '@tanstack/react-query';
import { approvalQuery } from '@/api/approvals';
import { approvalAtItem } from '@/features/approvals/current';
import { approvalSnapshot } from '@/features/approvals/snapshot';
import { StopControl, useStop } from '@/features/composer/StopControl';
import { describeStep } from '@/features/timeline/titles';
import type { Item, Turn } from '@/generated/api-types';
import { useFormatContext } from '@/lib/appearance';
import { holds } from '@/lib/permissions';
import { formatSpan, useRunningClock } from '@/lib/time';
import { runningTurn, threadReadOnly, useThreadPart } from '@/realtime/stores/thread';
import { useSocketStatus } from '@/realtime/live';
import { Tooltip } from '@/ui/Tooltip';

type ApprovalItem = Extract<Item, { kind: 'approval' }>;

/** Later completed work must never revive an older running step or a previous turn. */
export function activityWords(item: Item | undefined, turn: Turn, t: TFunction): { text: string; since: string | null } {
  if (turn.phase?.kind === 'compacting') return { text: t('thread.now.compacting'), since: turn.phase.since };
  if (item?.kind === 'approval') {
    const approval = approvalSnapshot(item.payload.approval, item.payload.approval_id);
    const target = approval?.kind === 'harness' && approval.request.request_kind === 'command'
      ? approval.request.detail.command : approval?.title;
    return { text: target === undefined ? t('thread.now.approval') : t('thread.now.approvalFor', { target }), since: turn.waiting_since ?? item.created_at };
  }
  if (item?.kind === 'tool_call') {
    const tool = item.payload.tool;
    const text = tool.kind === 'command' ? t('thread.now.running', { command: tool.command })
      : tool.kind === 'file_change' && tool.changes.length === 1 ? t('thread.now.writing', { path: tool.changes[0]?.path })
      : describeStep(item, t).text;
    return { text, since: item.payload.started_at };
  }
  if (item?.kind === 'reasoning') return { text: t('thread.now.thinking'), since: item.created_at };
  if (item?.kind === 'agent_message') return { text: t('thread.now.answering'), since: item.created_at };
  return { text: t(turn.state === 'starting' ? 'thread.now.starting' : 'thread.now.working'), since: turn.started_at };
}

export function useThreadNow(threadId: string) {
  const { t } = useTranslation();
  const format = useFormatContext();
  const thread = useThreadPart(threadId, (view) => view.thread);
  const turn = useThreadPart(threadId, runningTurn);
  const readOnly = useThreadPart(threadId, threadReadOnly);
  const socket = useSocketStatus();
  const fresh = useThreadPart(threadId, (view) => view.closed === null && view.error === null && view.loaded !== 'none');
  const live = socket.phase === 'ready' && fresh;
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useThreadPart(threadId, (view) => view.order);
  // Prepared only as admitted rows change, never by the one-second clock.
  const current = useMemo(() => order.flatMap((id) => {
    const item = items.get(id);
    return item !== undefined && item.turn_id === turn?.id ? [item] : [];
  }), [items, order, turn?.id]);
  const approvals = useMemo(() => current.filter((item): item is ApprovalItem => item.kind === 'approval' && item.status === 'pending'), [current]);
  const observed = useQueries({ queries: approvals.map((item) => ({ ...approvalQuery(item.payload.approval_id), enabled: false })) });
  const waiting = approvals.filter((item, index) => (approvalAtItem(item, observed[index]?.data)?.state ?? 'pending') === 'pending');
  const active = !readOnly && turn !== null && thread !== null && thread.lock === null && !thread.archived
    && (thread.state === 'running' || thread.state === 'starting' || thread.state === 'waiting_approval');
  const now = useRunningClock(active);
  const item = waiting[0] ?? current.findLast((one) => one.status === 'in_progress' && (one.kind === 'tool_call' || one.kind === 'reasoning' || one.kind === 'agent_message'));
  const activity = turn === null ? null : activityWords(item, turn, t);
  const since = activity?.since === null || activity?.since === undefined ? NaN : Date.parse(activity.since);
  const time = Number.isFinite(since) ? formatSpan(Math.max(0, now - since), format) : null;
  const label = activity === null ? '' : [activity.text, time].filter((part) => part !== null).join(' · ');
  return { active, label, text: activity?.text ?? '', toolRunning: current.some((one) => one.kind === 'tool_call' && one.status === 'in_progress'),
    toolCalls: current.filter((one) => one.kind === 'tool_call').length, approvals: waiting.length, thread, turn, live, readOnly };
}

/** What the thread is doing now, as the page reads it: the activity in words, its clock and whether a tool runs. */
export type ThreadNow = ReturnType<typeof useThreadNow>;

interface ThreadNowValue {
  now: ReturnType<typeof useThreadNow>;
  stop: ReturnType<typeof useStop>;
  mayStop: boolean;
}
const ThreadNowContext = createContext<ThreadNowValue | null>(null);
export const useProvidedThreadNow = () => useContext(ThreadNowContext);

/** The status line of the dock and whatever else of one screen names the activity share its words, its clock and its interruption. */
export function ThreadNowProvider({ threadId, children }: { threadId: string; children: ReactNode }) {
  const now = useThreadNow(threadId);
  const mayStop = now.thread !== null && holds(now.thread.access, 'thread.interact') && now.live && !now.readOnly;
  const stop = useStop(threadId, now.turn?.id ?? null, mayStop);
  return <ThreadNowContext.Provider value={{ now, stop, mayStop }}>{children}</ThreadNowContext.Provider>;
}

/** Stop, for the turn that runs: its words and failures are the composer's, which stands beside it. */
export function NowStop() {
  const { t } = useTranslation();
  const context = useProvidedThreadNow();
  if (context === null) return null;
  const { now, stop, mayStop } = context;
  const reason = !mayStop ? t(now.live ? 'composer.row.readOnly' : 'approvals.waitForLine') : null;
  return <Tooltip content={reason}><span><StopControl stop={stop} disabled={!mayStop} inlineFeedback={false} /></span></Tooltip>;
}
