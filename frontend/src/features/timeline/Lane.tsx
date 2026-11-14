/** One compact, indented conversation lane for a sub-agent or a person's aside. */
import { useMemo, type ReactNode, type Ref } from 'react';
import { TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ItemRow } from '@/features/timeline/ItemRow';
import { EmptyState } from '@/ui/EmptyState';
import { SkeletonRows } from '@/ui/Skeleton';

function LaneItems({ ids }: { ids: readonly string[] }) {
  return ids.map((id) => <li key={id}><ItemRow id={id} /></li>);
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
  /** Only actual message rows, excluding footer navigation, receive prompt scroll/focus. */
  itemsRef?: Ref<HTMLDivElement>;
  header?: ReactNode;
  intro?: ReactNode;
  history?: ReactNode;
  footer?: ReactNode;
}

/** All lanes use the ordinary item displays; only a person's aside supplies its own composer. */
export function ConversationLane({ kind, identity, label, activityLabel, context, ids, empty, loading = false, header, intro, history, footer, itemsRef }: ConversationLaneProps) {
  const compact = useMemo(() => ({ ...context, compact: true }), [context]);
  return <section aria-label={label} data-conversation-lane={kind} data-subagent-lane={kind === 'subagent' ? identity : undefined}
    data-side-lane={kind === 'side' ? identity : undefined} data-density="compact" className="flex min-w-0 flex-col gap-2 border-l-2 border-border-strong pl-3">
    {header !== undefined && <div className="flex flex-wrap items-center gap-2 text-sm">{header}</div>}
    {intro}
    {history}
    <TimelineContextValue.Provider value={compact}>
      <div ref={itemsRef} role="log" aria-label={activityLabel} className="flex min-w-0 flex-col gap-2">
        {ids.length === 0 ? loading ? <SkeletonRows rows={2} /> : <EmptyState compact title={empty} /> : <ol role="list" className="flex min-w-0 list-none flex-col gap-2"><LaneItems ids={ids} /></ol>}
      </div>
      {footer}
    </TimelineContextValue.Provider>
  </section>;
}
