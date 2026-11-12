/** Waiting decisions use their ordinary cards once, immediately above this screen's composer. */
import { useQueries } from '@tanstack/react-query';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { approvalQuery } from '@/api/approvals';
import { approvalAtItem } from '@/features/approvals/current';
import { NOT_PICKING, TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { recentTurns } from '@/features/timeline/rows';
import type { Item } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { useSocketStatus } from '@/realtime/live';
import { threadReadOnly, useThreadView } from '@/realtime/stores/thread';

type ApprovalItem = Extract<Item, { kind: 'approval' }>;

export function PinnedApprovals({ threadId }: { threadId: string }) {
  const { t } = useTranslation();
  const view = useThreadView(threadId);
  const preferences = useTimelinePreferences(threadId);
  const socket = useSocketStatus();
  const meId = useStore(client.session, (state) => state.userId);
  const readOnly = threadReadOnly(view);
  const items = useMemo(() => view.order.flatMap((id) => {
    const item = view.items.get(id);
    return item?.kind === 'approval' && item.status === 'pending' ? [item] : [];
  }), [view.order, view.items]);
  const observed = useQueries({ queries: items.map((item) => ({ ...approvalQuery(item.payload.approval_id), enabled: false })) });
  const pending = items.filter((item, index): item is ApprovalItem => (approvalAtItem(item, observed[index]?.data)?.state ?? 'pending') === 'pending');
  if (readOnly || view.closed !== null || view.thread === null || view.thread.archived || pending.length === 0) return null;
  const context: TimelineContext = { threadId, threadName: view.thread.name, detail: preferences.detail,
    layout: preferences.layout, setLayout: preferences.setLayout, recent: recentTurns(view.turns), meId,
    canInteract: holds(view.thread.access, 'thread.interact'), canManage: holds(view.thread.access, 'thread.manage'),
    live: socket.phase === 'ready' && view.error === null, readOnly: false, lit: null, selection: NOT_PICKING };
  return <TimelineContextValue.Provider value={context}><section aria-label={t('thread.pinnedApprovals')} className="flex min-w-0 flex-col gap-2" data-pinned-approvals>
    <h2 className="sr-only">{t('approvals.barWaiting', { count: pending.length })}</h2>
    {pending.map((item) => <div key={item.id} tabIndex={-1} data-pinned-approval={item.id}><ApprovalCard item={item} pinned /></div>)}
  </section></TimelineContextValue.Provider>;
}
