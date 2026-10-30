/** One compact, indented conversation lane for a sub-agent or a person's aside. */
import { memo, useMemo, type ReactNode, type Ref } from 'react';
import { useTranslation } from 'react-i18next';
import { TimelineContextValue, type TimelineContext, useTimelineContext } from '@/features/timeline/context';
import { AgentMessage } from '@/features/timeline/items/AgentMessage';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { AnnouncementCard } from '@/features/timeline/items/Announcement';
import { ErrorRow } from '@/features/timeline/items/Error';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { Plan } from '@/features/timeline/items/Plan';
import { Reasoning } from '@/features/timeline/items/Reasoning';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { useThreadPart } from '@/realtime/stores/thread';
import { EmptyState } from '@/ui/EmptyState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonRows } from '@/ui/Skeleton';

const LaneItem = memo(function LaneItem({ id, head }: { id: string; head: boolean }) {
  const { threadId, inspection } = useTimelineContext();
  const { t } = useTranslation();
  const item = useThreadPart(threadId, (view) => view.items.get(id));
  if (item === undefined) return null;
  switch (item.kind) {
    case 'user_message': return <UserMessage item={item} />;
    case 'agent_message': return <AgentMessage item={item} head={head} />;
    case 'reasoning': return <Reasoning item={item} />;
    case 'plan': return <Plan item={item} />;
    case 'tool_call': return <ToolCallRow threadId={threadId} id={id} />;
    case 'announcement': return <AnnouncementCard item={item} />;
    case 'marker': return <MarkerLine item={item} />;
    case 'error': return <ErrorRow item={item} />;
    case 'approval': return inspection === undefined ? <ApprovalCard item={item} /> : <InlineAlert tone="info">{t('trace.retainedApproval', { id: item.payload.approval_id })}</InlineAlert>;
  }
});

function LaneItems({ ids, agentHeads }: { ids: readonly string[]; agentHeads: boolean }) {
  const { threadId } = useTimelineContext();
  const items = useThreadPart(threadId, (view) => view.items);
  const spoken = new Set<string>();
  return ids.map((id) => {
    const item = items.get(id);
    const turn = item?.turn_id ?? id;
    const head = agentHeads && item?.kind === 'agent_message' && !spoken.has(turn);
    if (head) spoken.add(turn);
    return <LaneItem key={id} id={id} head={head} />;
  });
}

export interface ConversationLaneProps {
  kind: 'subagent' | 'side';
  identity: string;
  label: string;
  activityLabel: string;
  context: TimelineContext;
  ids: readonly string[];
  empty: string;
  loading?: boolean;
  agentHeads?: boolean;
  /** Only actual message rows, excluding footer navigation, receive prompt scroll/focus. */
  itemsRef?: Ref<HTMLDivElement>;
  header?: ReactNode;
  intro?: ReactNode;
  history?: ReactNode;
  footer?: ReactNode;
}

/** All lanes use the ordinary item displays; only a person's aside supplies its own composer. */
export function ConversationLane({ kind, identity, label, activityLabel, context, ids, empty, loading = false, agentHeads = false, header, intro, history, footer, itemsRef }: ConversationLaneProps) {
  const compact = useMemo(() => ({ ...context, compact: true }), [context]);
  return <section aria-label={label} data-conversation-lane={kind} data-subagent-lane={kind === 'subagent' ? identity : undefined}
    data-side-lane={kind === 'side' ? identity : undefined} data-density="compact" className="flex min-w-0 flex-col gap-2 border-l-2 border-border-strong pl-3">
    {header !== undefined && <div className="flex flex-wrap items-center gap-2 text-sm">{header}</div>}
    {intro}
    {history}
    <TimelineContextValue.Provider value={compact}>
      <div ref={itemsRef} role="log" aria-label={activityLabel} className="flex min-w-0 flex-col gap-2">
        {ids.length === 0 ? loading ? <SkeletonRows rows={2} /> : <EmptyState compact title={empty} /> : <LaneItems ids={ids} agentHeads={agentHeads} />}
      </div>
      {footer}
    </TimelineContextValue.Provider>
  </section>;
}
