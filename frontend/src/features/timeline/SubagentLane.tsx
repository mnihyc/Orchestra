/** Read-only child stream with shared renderers.
 *
 * Opens beneath the tool row that started it, indented behind one left border. It follows only that
 * reported child stream, with the ordinary item displays at compact density, and can open the same
 * stream in the workbench. Older child items are read on demand through the existing stream filter.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Bot } from 'lucide-react';
import { useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef, useState, type Ref } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchItems, fetchSubagentItems, OLDER_ITEMS } from '@/api/threads';
import { viewedItemQuery } from '@/api/threadHistory';
import { captureThreadRead, isCurrentThreadRead, selectThreadContinuation, threadQueryRead } from '@/api/threadRead';
import { ApiError } from '@/api/client';
import { NOT_PICKING, TimelineContextValue, type TimelineContext, useTimelineContext } from '@/features/timeline/context';
import { useRunning } from '@/features/running/live';
import { isOver } from '@/features/timeline/rows';
import { useAgentSwitch } from '@/features/thread/agentSwitch';
import type { RevealOptions, TimelineHandle } from '@/features/timeline/Timeline';
import { FindInConversation } from '@/features/timeline/FindInConversation';
import { isCancellation } from '@/realtime/sync';
import { ConversationLane } from '@/features/timeline/Lane';
import { useRecordedItem } from '@/features/timeline/historical';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { useViewShown, type ViewProps } from '@/features/workbench/views';
import type { Item, Turn } from '@/generated/api-types';
import { forgetThreadRead, useSocketStatus, useThreadFeed } from '@/realtime/live';
import { allTabs } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { timelineUi } from '@/features/timeline/state';
import { threadReadOnly, threadWriter, useThreadPart, useThreadView } from '@/realtime/stores/thread';
import { useThreadReadScope } from '@/realtime/useThreadRead';
import { ThreadReadProvider, useThreadReadRef } from '@/realtime/threadReadContext';
import { resultThreadRead } from '@/realtime/threadScope';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { Markdown } from '@/ui/Markdown';
import { SkeletonRows } from '@/ui/Skeleton';
import { ScrollArea } from '@/ui/ScrollArea';

type ToolItem = Extract<Item, { kind: 'tool_call' }>;
type ChildTool = Extract<ToolItem['payload']['tool'], { kind: 'subagent' }>;
const NO_ITEMS: readonly string[] = [];

interface ChildWindow { scope: AbortController | null; ids: readonly string[]; hasOlder: boolean; hasNewer: boolean }
interface ChildReveal { scope: AbortController | null; id: string; controller: AbortController; align: 'start' | 'center' }

function ChildStream({ item, tool, panel, ref: reference }: { item: ToolItem; tool: ChildTool; panel: boolean; ref?: Ref<TimelineHandle> | undefined }) {
  const { t } = useTranslation();
  const context = useTimelineContext();
  const format = useFormatContext();
  const switchTo = useAgentSwitch();
  const recorded = useRecordedItem(context.threadId, item);
  const view = useThreadView(context.threadId);
  const background = useRunning(context.threadId, context.inspection === undefined && !threadReadOnly(view));
  const observed = ((background.closed ? null : background.roster?.wake) ?? view.thread?.background)?.tasks.find((task) => task.kind === 'subagent' && task.item_id === item.id && task.turn_id === item.turn_id);
  const parent = item.turn_id === null ? undefined : view.turns.get(item.turn_id);
  const archived = recorded || parent !== undefined && isOver(parent);
  const state = observed === undefined ? tool.state === 'running' && archived ? 'recorded' : tool.state : observed.state === 'stopped' ? 'interrupted' : observed.state;
  const running = state === 'running';
  const now = useRunningClock(running);
  const tookMs = running ? Math.max(0, now - Date.parse(item.payload.started_at)) : item.payload.duration_ms;
  const threadId = context.threadId;
  const requestThread = context.inspection?.threadId ?? threadId;
  const read = useThreadReadRef(threadId);
  const tailIds = useThreadPart(threadId, (view) => view.streams.get(tool.child_stream) ?? NO_ITEMS);
  const scope = view.read?.controller ?? null;
  const [window, setWindow] = useState<ChildWindow | null>(null);
  const [target, setTarget] = useState<ChildReveal | null>(null);
  const [failure, setFailure] = useState<{ scope: AbortController | null; id: string; error: unknown } | null>(null);
  const [earlierRead, setEarlierRead] = useState<{ scope: AbortController | null; controller: AbortController } | null>(null);
  const loadingEarlier = earlierRead?.scope === scope && !earlierRead.controller.signal.aborted;
  const request = useRef<{ scope: AbortController | null; controller: AbortController } | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const focused = window?.scope === scope ? window : null;
  const revealed = target?.scope === scope ? target : null;
  const ids = focused?.ids ?? tailIds;
  useEffect(() => () => { if (request.current?.scope === scope) request.current.controller.abort(); }, [read, scope]);
  const sequence = useThreadPart(threadId, (view) => view.thread?.seq ?? null);
  const closed = useThreadPart(threadId, (view) => view.closed);
  useThreadReadScope(context.inspection === undefined ? read : null);
  const reading = threadQueryRead(read, ['subagent-items', requestThread, tool.child_stream]);
  const history = useInfiniteQuery({
    queryKey: reading.key,
    initialPageParam: null as number | null,
    queryFn: async ({ pageParam, signal }) => {
      await reading.capture(signal, pageParam !== null);
      return fetchSubagentItems(read, tool.child_stream, pageParam ?? (sequence ?? item.seq_updated) + 1, signal);
    },
    // History is fetched once; its query signal cancels only when the lane and panel both release it.
    // The shared thread feed appends new child-stream rows.
    staleTime: Infinity,
    getNextPageParam: (page) => page.has_older ? page.items[0]?.seq_created : undefined,
    enabled: context.inspection === undefined && closed === null && sequence !== null && tool.child_stream !== '' && reading.enabled,
    retry: false,
  });
  useEffect(() => {
    if (history.data !== undefined) threadWriter.apply(read, { items: history.data.pages.flatMap((page) => page.items), turns: history.data.pages.flatMap((page) => page.turns), segments: history.data.pages.flatMap((page) => page.segments) });
  }, [history.data, read]);
  const bring = (id: string, options: RevealOptions = {}) => {
    request.current?.controller.abort();
    const controller = new AbortController();
    request.current = { scope, controller };
    const captured = captureThreadRead(read);
    const current = () => !controller.signal.aborted && isCurrentThreadRead(captured, controller.signal);
    const show = () => { if (current()) { setFailure(null); setTarget({ scope, id, controller, align: options.align ?? 'center' }); } };
    if (ids.includes(id) && view.items.get(id)?.stream === tool.child_stream) { show(); return; }
    void fetchItems(read, { around: id, stream: tool.child_stream }, OLDER_ITEMS, controller.signal).then((page) => {
      if (!current()) return;
      if (!page.items.some((item) => item.id === id && item.stream === tool.child_stream)) throw new Error(t('timeline.itemUnavailable'));
      threadWriter.apply(read, { items: page.items, turns: page.turns, segments: page.segments }, captured);
      setWindow({ scope, ids: page.items.filter((item) => item.stream === tool.child_stream).map((item) => item.id), hasOlder: page.has_older, hasNewer: page.has_newer });
      show();
    }).catch((error: unknown) => { if (current() && !isCancellation(error)) setFailure({ scope, id, error }); });
  };
  const jump = () => {
    request.current?.controller.abort(); setWindow(null); setTarget(null); setFailure(null);
    requestAnimationFrame(() => root.current?.lastElementChild?.scrollIntoView({ block: 'end' }));
  };
  const earlier = async () => {
    if (focused === null) { await history.fetchNextPage(); return; }
    const first = view.items.get(focused.ids[0] ?? '');
    if (first === undefined || loadingEarlier) return;
    request.current?.controller.abort();
    const controller = new AbortController(); request.current = { scope, controller };
    const captured = captureThreadRead(read); setEarlierRead({ scope, controller });
    try {
      const page = await fetchItems(read, { before: first.seq_created, stream: tool.child_stream }, OLDER_ITEMS, controller.signal);
      if (!isCurrentThreadRead(captured, controller.signal)) return;
      threadWriter.apply(read, { items: page.items, turns: page.turns, segments: page.segments }, captured);
      setWindow((held) => held?.scope === scope ? { ...held, ids: [...new Set([...page.items.map((item) => item.id), ...held.ids])], hasOlder: page.has_older } : held);
    } catch (error) { if (isCurrentThreadRead(captured, controller.signal) && !isCancellation(error)) setFailure({ scope, id: first.id, error }); }
    finally { setEarlierRead((held) => held?.controller === controller ? null : held); }
  };
  useImperativeHandle(reference, () => ({ scrollToItem: bring, scrollToEnd: jump, visibleItemIds: () => new Set(ids) }));
  const visibleTargets = useMemo(() => revealed === null ? context.revealed : new Set([...(context.revealed ?? []), revealed.id]), [context.revealed, revealed]);
  const childContext = useMemo<TimelineContext>(() => {
    const turns = new Map<string, Turn>();
    const own = new Set(ids.flatMap((id) => { const row = view.items.get(id); return row?.turn_id === null || row?.turn_id === undefined ? [] : [row.turn_id]; }));
    for (const id of own) {
      const source = view.turns.get(id);
      if (source === undefined) continue;
      const duration = id === item.turn_id ? item.payload.duration_ms : null;
      const start = duration === null ? null : item.payload.started_at;
      const completed = running ? null : duration === null ? item.updated_at : new Date(Date.parse(item.payload.started_at) + duration).toISOString();
      turns.set(id, { ...source, state: running ? 'running' : state === 'completed' ? 'completed' : state === 'failed' ? 'failed' : 'interrupted',
        started_at: start, completed_at: completed, waited_ms: 0, waiting_since: null, phase: null, error: null, interrupted_by: null,
        model: tool.model === '' ? source.model : tool.model ?? source.model, model_display: tool.model === '' ? source.model_display : tool.model ?? source.model_display, effort: null, params: null, plan: false,
        usage: source.usage_detail?.by_subagent.find((share) => share.stream === tool.child_stream)?.tokens ?? null, usage_detail: null,
        context: null, command_cleanup: null, before_snapshot_id: null, files_changed: null, limit_bypass: [],
      });
    }
    return { ...context, ...(visibleTargets === undefined ? {} : { revealed: visibleTargets }), readOnly: true, canInteract: false, canManage: false, pinApprovals: false, selection: NOT_PICKING,
      childStreams: [...(context.childStreams ?? []), tool.child_stream], conversation: { order: ids, turns,
        hasOlder: focused?.hasOlder ?? history.hasNextPage, hasNewer: focused?.hasNewer ?? false, stream: tool.child_stream, ...(state === 'recorded' || state === 'lost' ? { ending: state } : {}) } };
  }, [context, ids, view.items, view.turns, item.turn_id, item.payload.duration_ms, item.payload.started_at, item.updated_at, running, state, tool.model, tool.child_stream, history.hasNextPage, focused, visibleTargets]);
  if (closed !== null) return <p className="text-sm text-text-muted">{t('threadViews.unavailable')}</p>;
  if (history.error instanceof ApiError && (history.error.status === 403 || history.error.status === 404)) return <ErrorState compact error={history.error} />;
  const name = tool.name ?? t('timeline.step.subagentUnnamed');
  return <ConversationLane kind="subagent" identity={tool.child_stream} label={t('timeline.subagent.lane', { name })} activityLabel={t('timeline.subagent.activity', { name })}
    context={childContext} itemsRef={root} target={revealed} ids={ids} loading={history.isPending && context.inspection === undefined} empty={t('timeline.subagent.empty')}
    header={<>
      {panel && <><Bot className="shrink-0 text-text-muted" aria-hidden /><span className="wrap-anywhere font-medium">{name}</span></>}
      {tool.role !== null && tool.role !== '' && <span className="text-text-muted">{t('timeline.subagent.role', { role: tool.role })}</span>}
      {tool.model !== undefined && tool.model !== null && tool.model !== '' && <span data-subagent-model className="wrap-anywhere text-text-muted">{tool.model}</span>}
      {!panel && switchTo !== null && <Button size="sm" variant="ghost" onClick={() => switchTo(item.id)}>{t('thread.agents.open')}</Button>}
      {panel && <span className="text-text-muted">{t(state === 'recorded' ? 'timeline.step.recordedRunning' : state === 'lost' ? 'backgroundWake.states.lost' : `timeline.step.subagentState.${state}`)}</span>}
      {panel && tookMs !== null && tookMs >= 1000 && <span data-subagent-time className="text-xs text-text-muted tabular-nums">{formatSpan(tookMs, format)}</span>}
    </>}
    intro={tool.prompt !== null && tool.prompt !== '' ? <Markdown text={tool.prompt} compact /> : undefined}
    history={<>{history.isError && <ErrorState compact error={history.error} onRetry={() => void history.refetch()} />}
      {(focused?.hasOlder ?? history.hasNextPage) && <Button size="sm" variant="ghost" pending={history.isFetchingNextPage || loadingEarlier} onClick={() => void earlier()}>{t('timeline.subagent.older')}</Button>}
      {focused?.hasNewer === true && <Button size="sm" variant="ghost" onClick={jump}>{t('timeline.jump')}</Button>}
      {failure?.scope === scope && <ErrorState compact error={failure.error} onRetry={() => bring(failure.id)} />}</>}
  />;
}

export function SubagentLane({ item, panel = false, ref: reference }: { item: ToolItem; panel?: boolean; ref?: Ref<TimelineHandle> | undefined }) {
  const context = useTimelineContext();
  const { t } = useTranslation();
  const tool = item.payload.tool;
  if (tool.kind !== 'subagent' || item.thread_id !== (context.inspection?.threadId ?? context.threadId)
    || context.childStreams?.includes(tool.child_stream) === true) return null;
  if (tool.child_stream === '') return <p role="status" className="text-sm text-text-muted">{t('timeline.subagent.unavailable')}</p>;
  return <ChildStream item={item} tool={tool} panel={panel} ref={reference} />;
}

/**
 * One sub-agent's own conversation, read-only: the lane of the step that started it, with its name, state and clock, the prompt it was
 * given and its items. The side panel shows it in a scrolling region of its own; the screen of a thread shows it in place of the main
 * conversation.
 */
export function SubagentConversation({ threadId, itemId, frame = 'page', ref: reference }: { threadId: string; itemId: string; frame?: 'page' | 'panel'; ref?: Ref<TimelineHandle> | undefined }) {
  const { t } = useTranslation();
  const own = useRef<TimelineHandle>(null);
  const root = useRef<HTMLDivElement>(null);
  useImperativeHandle(reference, () => ({ scrollToItem: (id, options) => own.current?.scrollToItem(id, options), scrollToEnd: () => own.current?.scrollToEnd(), visibleItemIds: () => own.current?.visibleItemIds() ?? new Set() }));
  const preferences = useTimelinePreferences(threadId);
  const read = useThreadReadRef(threadId);
  const view = useThreadView(threadId);
  const socket = useSocketStatus();
  // What a conversation is told by default (how the work is shown) comes from the context, not from this lane.
  const base = useTimelineContext();
  const held = view.items.get(itemId);
  const parentOptions = viewedItemQuery(read, itemId);
  const parent = useQuery({ ...parentOptions, enabled: held === undefined && view.thread !== null && view.closed === null && parentOptions.enabled, retry: false });
  const item = held ?? parent.data;
  if (view.closed !== null) return <p className="p-3 text-sm text-text-muted">{t('threadViews.unavailable')}</p>;
  if (view.error !== null && view.thread === null) return <ErrorState compact error={view.error} />;
  if (parent.isError && item === undefined) return <ErrorState compact error={parent.error} onRetry={() => void parent.refetch()} />;
  if (item === undefined) return <SkeletonRows rows={3} />;
  if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'subagent') return <p className="p-3 text-sm">{t('timeline.subagent.unavailable')}</p>;
  const context: TimelineContext = { ...base, threadId, threadName: view.thread?.name ?? '', detail: preferences.detail, hideHistory: preferences.hideHistory, layout: preferences.layout, setLayout: preferences.setLayout, meId: null, canInteract: false, canManage: false, readOnly: true, live: socket.phase === 'ready', lit: null, selection: NOT_PICKING, compact: true };
  const lane = <TimelineContextValue.Provider value={context}><SubagentLane item={item} panel ref={own} /></TimelineContextValue.Provider>;
  return frame === 'page' ? <div data-subagent-conversation={item.id}>{lane}</div>
    : <ScrollArea ref={root} role="region" aria-label={t('timeline.subagent.panelTitle')} tabIndex={0} className="focus-inset h-full p-3" data-subagent-panel={item.id}><FindInConversation threadId={threadId} stream={item.payload.tool.child_stream} timeline={own} root={root} />{lane}</ScrollArea>;
}

function Panel({ threadId, itemId, continuationId }: { threadId: string; itemId: string; continuationId: string | null }) {
  const read = useThreadReadRef(threadId);
  useLayoutEffect(() => selectThreadContinuation(read, continuationId), [read, continuationId]);
  useThreadFeed(read);
  return <SubagentConversation threadId={threadId} itemId={itemId} frame="panel" />;
}

export function SubagentPanel({ tab }: ViewProps) {
  const shown = useViewShown();
  const threadId = tab.of?.thread;
  const itemId = tab.of?.item;
  return !shown || threadId === undefined || itemId === undefined ? null : <OwnedPanel threadId={threadId} itemId={itemId}
    tabId={tab.id} continuationId={tab.of?.continuation ?? null} />;
}

function OwnedPanel({ threadId, itemId, tabId, continuationId }: {
  threadId: string; itemId: string; tabId: string; continuationId: string | null;
}) {
  const read = useMemo(() => resultThreadRead(threadId, tabId), [threadId, tabId]);
  useEffect(() => () => {
    queueMicrotask(() => {
      if (!allTabs(useWorkbench.getState().layout).some((tab) => tab.id === tabId && tab.of?.thread === threadId)) {
        forgetThreadRead(read); timelineUi.forget(read);
      }
    });
  }, [read, tabId, threadId]);
  return <ThreadReadProvider read={read}><Panel threadId={threadId} itemId={itemId} continuationId={continuationId} /></ThreadReadProvider>;
}
