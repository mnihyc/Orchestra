/** Read-only child stream with shared renderers.
 *
 * Opens beneath the tool row that started it, indented behind one left border. It follows only that
 * reported child stream, with the ordinary item displays at compact density, and can open the same
 * stream in the workbench. Older child items are read on demand through the existing stream filter.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Bot } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchSubagentItems } from '@/api/threads';
import { viewedItemQuery } from '@/api/threadHistory';
import { selectThreadContinuation, threadQueryRead } from '@/api/threadRead';
import { ApiError } from '@/api/client';
import { NOT_PICKING, TimelineContextValue, type TimelineContext, useTimelineContext } from '@/features/timeline/context';
import { ConversationLane } from '@/features/timeline/Lane';
import { useRecordedItem } from '@/features/timeline/historical';
import { recentTurns } from '@/features/timeline/rows';
import type { ViewProps } from '@/features/workbench/views';
import type { Item } from '@/generated/api-types';
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
import { holds } from '@/lib/permissions';
import { useFormatContext } from '@/lib/appearance';
import { formatSpan, useRunningClock } from '@/lib/time';
import { Markdown } from '@/ui/Markdown';
import { SkeletonRows } from '@/ui/Skeleton';
import { ScrollArea } from '@/ui/ScrollArea';

type ToolItem = Extract<Item, { kind: 'tool_call' }>;
type ChildTool = Extract<ToolItem['payload']['tool'], { kind: 'subagent' }>;
const NO_ITEMS: readonly string[] = [];

function ChildStream({ item, tool, panel }: { item: ToolItem; tool: ChildTool; panel: boolean }) {
  const { t } = useTranslation();
  const context = useTimelineContext();
  const format = useFormatContext();
  const recorded = useRecordedItem(context.threadId, item);
  const running = tool.state === 'running' && !recorded;
  const now = useRunningClock(running);
  const tookMs = running ? Math.max(0, now - Date.parse(item.payload.started_at)) : item.payload.duration_ms;
  const threadId = context.threadId;
  const requestThread = context.inspection?.threadId ?? threadId;
  const read = useThreadReadRef(threadId);
  const ids = useThreadPart(threadId, (view) => view.streams.get(tool.child_stream) ?? NO_ITEMS);
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
    if (history.data !== undefined) threadWriter.apply(read, { items: history.data.pages.flatMap((page) => page.items) });
  }, [history.data, read]);
  const childContext = useMemo<TimelineContext>(() => ({ ...context, selection: NOT_PICKING, childStreams: [...(context.childStreams ?? []), tool.child_stream] }), [context, tool.child_stream]);
  if (closed !== null) return <p className="text-sm text-text-muted">{t('threadViews.unavailable')}</p>;
  if (history.error instanceof ApiError && (history.error.status === 403 || history.error.status === 404)) return <ErrorState compact error={history.error} />;
  const name = tool.name ?? t('timeline.step.subagentUnnamed');
  return <ConversationLane kind="subagent" identity={tool.child_stream} label={t('timeline.subagent.lane', { name })} activityLabel={t('timeline.subagent.activity', { name })}
    context={childContext} ids={ids} loading={history.isPending && context.inspection === undefined} empty={t('timeline.subagent.empty')}
    header={<>
      {panel && <><Bot className="shrink-0 text-text-muted" aria-hidden /><span className="wrap-anywhere font-medium">{name}</span></>}
      {tool.role !== null && tool.role !== '' && <span className="text-text-muted">{t('timeline.subagent.role', { role: tool.role })}</span>}
      {tool.model !== undefined && tool.model !== '' && <span data-subagent-model className="wrap-anywhere text-text-muted">{tool.model}</span>}
      {panel && <span className="text-text-muted">{t(recorded && tool.state === 'running' ? 'timeline.step.recordedRunning' : `timeline.step.subagentState.${tool.state}`)}</span>}
      {panel && tookMs !== null && tookMs >= 1000 && <span data-subagent-time className="text-xs text-text-muted tabular-nums">{formatSpan(tookMs, format)}</span>}
    </>}
    intro={tool.prompt !== null && tool.prompt !== '' ? <Markdown text={tool.prompt} compact /> : undefined}
    history={<>{history.isError && <ErrorState compact error={history.error} onRetry={() => void history.refetch()} />}
      {history.hasNextPage && <Button size="sm" variant="ghost" pending={history.isFetchingNextPage} onClick={() => void history.fetchNextPage()}>{t('timeline.subagent.older')}</Button>}</>}
  />;
}

export function SubagentLane({ item, panel = false }: { item: ToolItem; panel?: boolean }) {
  const context = useTimelineContext();
  const tool = item.payload.tool;
  if (tool.kind !== 'subagent' || context.childStreams?.includes(tool.child_stream) === true) return null;
  return <ChildStream item={item} tool={tool} panel={panel} />;
}

function Panel({ threadId, itemId, continuationId }: { threadId: string; itemId: string; continuationId: string | null }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  useLayoutEffect(() => selectThreadContinuation(read, continuationId), [read, continuationId]);
  useThreadFeed(read);
  const view = useThreadView(threadId);
  const socket = useSocketStatus();
  const held = view.items.get(itemId);
  const parentOptions = viewedItemQuery(read, itemId);
  const parent = useQuery({ ...parentOptions, enabled: held === undefined && view.thread !== null && view.closed === null && parentOptions.enabled, retry: false });
  const item = held ?? parent.data;
  if (view.closed !== null) return <p className="p-3 text-sm text-text-muted">{t('threadViews.unavailable')}</p>;
  if (view.error !== null && view.thread === null) return <ErrorState compact error={view.error} />;
  if (parent.isError && item === undefined) return <ErrorState compact error={parent.error} onRetry={() => void parent.refetch()} />;
  if (item === undefined) return <SkeletonRows rows={3} />;
  if (item.kind !== 'tool_call' || item.payload.tool.kind !== 'subagent') return <p className="p-3 text-sm">{t('timeline.subagent.unavailable')}</p>;
  const readOnly = threadReadOnly(view);
  const context: TimelineContext = { threadId, threadName: view.thread?.name ?? '', detail: 'steps', recent: recentTurns(view.turns), layout: 'one', setLayout: () => undefined, meId: null, canInteract: !readOnly && holds(view.thread?.access, 'thread.interact'), canManage: !readOnly && holds(view.thread?.access, 'thread.manage'), readOnly, live: socket.phase === 'ready', lit: null, selection: NOT_PICKING, compact: true };
  return <TimelineContextValue.Provider value={context}><ScrollArea role="region" aria-label={t('timeline.subagent.panelTitle')} tabIndex={0} className="focus-inset h-full p-3" data-subagent-panel={item.id}><SubagentLane item={item} panel /></ScrollArea></TimelineContextValue.Provider>;
}

export function SubagentPanel({ tab }: ViewProps) {
  const threadId = tab.of?.thread;
  const itemId = tab.of?.item;
  return threadId === undefined || itemId === undefined ? null : <OwnedPanel threadId={threadId} itemId={itemId}
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
