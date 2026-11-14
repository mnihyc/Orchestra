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
import { currentTurnOver, runningTurn, threadReadOnly, threadStopReadOnly, useThreadPart } from '@/realtime/stores/thread';
import { useSocketStatus } from '@/realtime/live';
import { Tooltip } from '@/ui/Tooltip';
import { StateChip, type WorkState } from '@/ui/StateChip';
import { useCompactControls } from '@/features/workbench/Narrow';

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
  const stopReadOnly = useThreadPart(threadId, threadStopReadOnly);
  const over = useThreadPart(threadId, currentTurnOver);
  const socket = useSocketStatus();
  const fresh = useThreadPart(threadId, (view) => view.closed === null && view.error === null);
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
  const active = !stopReadOnly && !over && thread !== null && thread.lock === null && !thread.archived
    && (thread.state === 'running' || thread.state === 'starting' || thread.state === 'waiting_approval');
  const now = useRunningClock(active);
  const item = waiting[0] ?? current.findLast((one) => one.status === 'in_progress' && (one.kind === 'tool_call' || one.kind === 'reasoning' || one.kind === 'agent_message'));
  const activity = turn === null ? thread === null ? null : { text: t(thread.state === 'starting' ? 'thread.now.starting' : 'thread.now.working'), since: null } : activityWords(item, turn, t);
  const since = activity?.since === null || activity?.since === undefined ? NaN : Date.parse(activity.since);
  const time = Number.isFinite(since) ? formatSpan(Math.max(0, now - since), format) : null;
  const label = activity === null ? '' : [activity.text, time].filter((part) => part !== null).join(' · ');
  return { active, label, toolCalls: current.filter((one) => one.kind === 'tool_call').length, approvals: waiting.length, thread, turn, live, readOnly, stopReadOnly };
}

interface ThreadNowValue {
  now: ReturnType<typeof useThreadNow>;
  stop: ReturnType<typeof useStop>;
  mayStop: boolean;
}
const ThreadNowContext = createContext<ThreadNowValue | null>(null);
export const useProvidedThreadNow = () => useContext(ThreadNowContext);

/** Header and its line share the same activity, clock and interruption in one screen. */
export function ThreadNowProvider({ threadId, children }: { threadId: string; children: ReactNode }) {
  const now = useThreadNow(threadId);
  const mayStop = now.thread !== null && holds(now.thread.access, 'thread.interact') && now.live && !now.stopReadOnly;
  const stop = useStop(threadId, now.turn?.id ?? now.thread?.current_turn_id ?? null, mayStop, now.thread?.state === 'starting');
  return <ThreadNowContext.Provider value={{ now, stop, mayStop }}>{children}</ThreadNowContext.Provider>;
}

function NowStop() {
  const { t } = useTranslation();
  const context = useProvidedThreadNow();
  if (context === null) return null;
  const { now, stop, mayStop } = context;
  const reason = !mayStop ? t(now.live ? 'composer.row.readOnly' : 'approvals.waitForLine') : null;
  return <Tooltip content={reason}><span><StopControl stop={stop} disabled={!mayStop} /></span></Tooltip>;
}

export function ThreadNowChip({ state }: { state: WorkState }) {
  const { t } = useTranslation();
  const context = useProvidedThreadNow();
  if (!context?.now.active) return null;
  const { now } = context;
  const counts = `${t('thread.viewModes.toolCalls', { count: now.toolCalls })} · ${t('thread.now.approvals', { count: now.approvals })}`;
  return <div data-thread-now-chip className="flex min-w-0 max-w-full flex-wrap items-center gap-2">
    <Tooltip content={counts}><span role="img" tabIndex={0} aria-label={`${now.label} · ${counts}`} className="min-w-0 max-w-full"><StateChip state={state} label={now.label} /></span></Tooltip>
    <NowStop />
  </div>;
}

export function NowLine({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const compact = useCompactControls();
  const context = useProvidedThreadNow();
  if (context === null) return <ThreadNowProvider threadId={threadId}><NowLine threadId={threadId} /></ThreadNowProvider>;
  const { now } = context;
  if (!now.active || compact || now.thread?.id !== threadId) return null;
  return <div data-thread-now role="status" className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 border-b border-border px-4 py-1 text-sm text-text-muted">
    <span className="min-w-0 flex-1 wrap-anywhere">{now.label}</span>
    <span>{t('thread.viewModes.toolCalls', { count: now.toolCalls })} · {t('thread.now.approvals', { count: now.approvals })}</span>
    <NowStop />
  </div>;
}
