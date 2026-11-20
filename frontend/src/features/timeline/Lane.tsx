/** One compact, indented conversation lane for a sub-agent or a person's aside. */
import { useEffect, useMemo, useRef, type ReactNode, type Ref } from 'react';
import { TimelineContextValue, type TimelineContext } from '@/features/timeline/context';
import { ConversationRow } from '@/features/timeline/ConversationRow';
import { buildRows, isStep } from '@/features/timeline/rows';
import { timelineUi } from '@/features/timeline/state';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useThreadView } from '@/realtime/stores/thread';
import { EmptyState } from '@/ui/EmptyState';
import { SkeletonRows } from '@/ui/Skeleton';

export interface ConversationLaneProps {
  target?: { id: string; controller: AbortController; align: 'start' | 'center' } | null;
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
export function ConversationLane({ kind, identity, label, activityLabel, context, ids, empty, loading = false, header, intro, history, footer, itemsRef, target }: ConversationLaneProps) {
  const view = useThreadView(context.threadId);
  const read = useThreadReadRef(context.threadId);
  const root = useRef<HTMLElement>(null);
  const brought = useRef<typeof target>(undefined);
  const compact = useMemo(() => ({ ...context, compact: true, conversation: context.conversation ?? {
    order: ids, turns: view.turns, hasOlder: view.hasOlder, hasNewer: view.hasNewer,
  } }), [context, ids, view.turns, view.hasOlder, view.hasNewer]);
  const rows = useMemo(() => buildRows({ ...view, ...compact.conversation, order: ids, turns: new Map(compact.conversation.turns), openedSeq: null, openedReadSeq: null }, {
    detail: context.detail, hideHistory: context.hideHistory ?? false, meId: context.meId, ...(context.revealed === undefined ? {} : { revealed: context.revealed }),
  }).filter((row) => row.kind !== 'divider'), [view, compact.conversation, ids, context.detail, context.hideHistory, context.meId, context.revealed]);
  useEffect(() => {
    if (target === undefined || target === null || target.controller.signal.aborted || brought.current === target) return;
    const destination = rows.find((row) => row.kind === 'item' && row.id === target.id || (row.kind === 'work' || row.kind === 'markers' || row.kind === 'start') && row.ids.includes(target.id));
    if (destination === undefined) return;
    const item = view.items.get(target.id);
    if (item !== undefined && isStep(item)) timelineUi.revealStep(read, item.id);
    if (destination.kind === 'work') timelineUi.setStep(read, destination.key, true);
    let frame = 0;
    const viewport = root.current?.closest<HTMLElement>('[data-scroll-area]') ?? null;
    const events = ['wheel', 'touchstart', 'pointerdown', 'keydown'] as const;
    const stop = () => {
      observer.disconnect(); cancelAnimationFrame(frame);
      for (const event of events) viewport?.removeEventListener(event, cancel);
    };
    const cancel = () => { target.controller.abort(); stop(); };
    const locate = () => {
      if (target.controller.signal.aborted) { stop(); return; }
      const element = root.current?.querySelector<HTMLElement>(`[data-item="${CSS.escape(target.id)}"]`);
      if (element === undefined || element === null) return;
      if (viewport === null || typeof viewport.scrollTo !== 'function') element.scrollIntoView({ block: target.align, behavior: 'auto' });
      else {
        const box = element.getBoundingClientRect(), visible = viewport.getBoundingClientRect();
        viewport.scrollTo({ top: Math.max(0, viewport.scrollTop + box.top - visible.top + (target.align === 'center' ? box.height / 2 - visible.height / 2 : 0)), behavior: 'auto' });
      }
      brought.current = target; stop();
    };
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(locate); };
    const observer = new MutationObserver(schedule);
    if (root.current !== null) observer.observe(root.current, { childList: true, subtree: true });
    for (const event of events) viewport?.addEventListener(event, cancel, { passive: true });
    schedule();
    return stop;
  }, [target, rows, view.items, read]);
  return <section ref={root} aria-label={label} data-conversation-lane={kind} data-subagent-lane={kind === 'subagent' ? identity : undefined}
    data-side-lane={kind === 'side' ? identity : undefined} data-density="compact" className="flex min-w-0 flex-col gap-2 border-l-2 border-border-strong pl-3">
    {header !== undefined && <div className="flex flex-wrap items-center gap-2 text-sm">{header}</div>}
    {intro}
    {history}
    <TimelineContextValue.Provider value={compact}>
      <div ref={itemsRef} role="log" aria-label={activityLabel} className="flex min-w-0 flex-col gap-2">
        {rows.length === 0 ? loading ? <SkeletonRows rows={2} /> : <EmptyState compact title={empty} /> : <ol role="list" className="flex min-w-0 list-none flex-col gap-2">{rows.map((row) => <li key={row.key}><ConversationRow row={row} /></li>)}</ol>}
      </div>
      {footer}
    </TimelineContextValue.Provider>
  </section>;
}
