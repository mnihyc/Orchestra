/** Virtualized main stream, history loading and stable scroll anchors.
 *
 * The conversation of a thread: its main stream in the order the items were made, in a column of at
 * most 820 px, drawn by the rows in view so that thousands of items scroll as smoothly as ten. It
 * follows new content while its reader is at the end; when they are not, it leaves them where they are
 * and says how much is new below. Scrolling to the top reads older items without moving what is on
 * screen. A link that names an item brings it into view and lights it for two seconds. The newest item
 * is marked read two seconds after it has been on screen with the tab in front.
 */
import { useCompactControls } from '@/features/workbench/Narrow';
import { ArrowDown, Bot, LoaderCircle } from 'lucide-react';
import { memo, type ReactNode, type Ref, useCallback, useEffect, useId, useImperativeHandle, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { StateLine } from '@/ui/StateLine';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import type { ThreadSync } from '@/generated/api-types';
import { fetchItems, markRead } from '@/api/threads';
import { captureThreadRead, isCurrentThreadRead } from '@/api/threadRead';
import { type ItemSelection, NOT_PICKING, type PromptFocus, type SelectPromptVersion, type TimelineContext, TimelineContextValue, useTimelineContext } from '@/features/timeline/context';
import { usePromptFocus } from '@/features/timeline/usePromptFocus';
import { NewLine } from '@/features/timeline/Catchup';
import { AgentMessage } from '@/features/timeline/items/AgentMessage';
import { AnnouncementCard } from '@/features/timeline/items/Announcement';
import { BeforeOrchHeading } from '@/features/timeline/items/BeforeOrch';
import { ApprovalCard } from '@/features/timeline/items/Approval';
import { ErrorRow } from '@/features/timeline/items/Error';
import { MarkerLine } from '@/features/timeline/items/Marker';
import { Plan } from '@/features/timeline/items/Plan';
import { Reasoning } from '@/features/timeline/items/Reasoning';
import { ToolCallRow } from '@/features/timeline/items/ToolCall';
import { UserMessage } from '@/features/timeline/items/UserMessage';
import { useTimelinePreferences } from '@/features/timeline/preferences';
import { buildRows, estimateRow, type Row, recentTurns } from '@/features/timeline/rows';
import { StatusLine } from '@/features/timeline/StatusLine';
import { SideThreadLaneRow } from '@/features/timeline/SideThreadLane';
import { useThreadUi } from '@/features/timeline/state';
import { keepTimelinePosition, readTimelinePosition } from './position';
import { WorkFold } from '@/features/timeline/Steps';
import { TurnFooter } from '@/features/timeline/TurnFooter';
import { reloadThread, useSocketStatus } from '@/realtime/live';
import { ThreadReadProvider, useThreadReadRef } from '@/realtime/threadReadContext';
import { resultThreadRead, type ThreadReadRef } from '@/realtime/threadScope';
import { threadReadOnly, threadViewOf, threadWriter, useThreadPart, useThreadView } from '@/realtime/stores/thread';
import { holds } from '@/lib/permissions';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { InlineAlert } from '@/ui/InlineAlert';
import { SkeletonGroup, Skeleton } from '@/ui/Skeleton';
import { VirtualList, type VirtualListHandle } from '@/ui/VirtualList';

/** How long an item that a link named stays lit. */
const LIT_MS = 2_000;
/** How long the newest item has to have been on screen before it is marked read. */
const READ_AFTER_MS = 2_000;

/** The existing rows drawn from a stored snapshot, without live following, read markers, queues or conversation acts. */
export function StoredTimeline({ snapshot }: { snapshot: ThreadSync }) {
  const owner = useId();
  const read = useMemo(() => resultThreadRead(snapshot.thread.id, `inspection:${owner}`), [snapshot.thread.id, owner]);
  return <ThreadReadProvider read={read}><StoredHistory snapshot={snapshot} read={read} /></ThreadReadProvider>;
}

function StoredHistory({ snapshot, read }: { snapshot: ThreadSync; read: ThreadReadRef }) {
  const key = snapshot.thread.id;
  const preferences = useTimelinePreferences();
  const view = useThreadView(key);
  const ui = useThreadUi(key);
  useEffect(() => {
    threadWriter.reset(read, snapshot, snapshot.has_older);
    return () => threadWriter.evict(read);
  }, [read, snapshot]);
  const rows = buildRows(view, { detail: preferences.detail, hideHistory: preferences.hideHistory, foldOpened: ui.folds, meId: null }).filter((row) => row.kind !== 'divider' && row.kind !== 'status');
  const context: TimelineContext = { threadId: key, threadName: snapshot.thread.name,
    inspection: { threadId: snapshot.thread.id, ...(snapshot.projection === undefined || snapshot.projection === null ? {} : { continuationId: snapshot.projection.viewed_continuation_id }) },
    detail: preferences.detail, recent: recentTurns(view.turns), layout: preferences.layout, setLayout: preferences.setLayout, meId: null, canInteract: false, canManage: false, live: false, lit: null, selection: NOT_PICKING };
  return <TimelineContextValue.Provider value={context}><div className="mx-auto flex w-full max-w-(--reading-width) flex-col gap-6 px-2 py-4" data-stored-timeline>{rows.map((row) => <RowView key={row.key} row={row} />)}</div></TimelineContextValue.Provider>;
}

export interface TimelineHandle {
  /** Brings an item into view (reading it from the server if it is not held). */
  scrollToItem: (itemId: string) => void;
  /** Brings the newest end into view. */
  scrollToEnd: () => void;
  /** Rows in view, for the bar that says an approval waits off screen. */
  visibleItemIds: () => ReadonlySet<string>;
}

export interface TimelineProps {
  threadId: string;
  /** A retained tab behind another does not mark its items read. */
  shown?: boolean;
  /** An item the address names. */
  item: string | null;
  /** The other items the address names, lit with it: a link to a selection. */
  items?: readonly string[] | null;
  /** What stands in an empty conversation after "Send the first message": the binding summary, or the way to choose one. */
  empty: ReactNode;
  /** The ids of the items in view, as they change. */
  onVisible?: (ids: ReadonlySet<string>) => void;
  /** The items being picked, to forward, quote or discuss: the screen holds them, the items show them. */
  selection?: ItemSelection;
  onVersionSelect?: SelectPromptVersion | undefined;
  focusPrompt?: PromptFocus | null | undefined;
  /** Version browsing keeps its prompt in place; ordinary permalinks still briefly identify their target. */
  highlightLink?: boolean;
  ref?: Ref<TimelineHandle>;
}

function Skeletons() {
  return (
    <SkeletonGroup className="mx-auto flex w-full max-w-(--reading-width) flex-col gap-6 px-4 py-6">
      {[0, 1, 2].map((row) => (
        <div key={row} className="flex gap-3">
          <Skeleton className="size-(--avatar-message) rounded-full" />
          <div className="flex flex-1 flex-col gap-2">
            <Skeleton className="h-3 w-(--skeleton-line)" />
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-1/2" />
          </div>
        </div>
      ))}
    </SkeletonGroup>
  );
}

export function Timeline({ threadId, shown = true, item: linked, items: alsoLinked = null, empty, onVisible, selection = NOT_PICKING, onVersionSelect, focusPrompt, highlightLink = true, ref }: TimelineProps) {
  const compact = useCompactControls();
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(threadId);
  const preferences = useTimelinePreferences();
  const ui = useThreadUi(threadId);
  const meId = useStore(client.session, (state) => state.userId);
  const socket = useSocketStatus();
  const thread = view.thread;
  const readOnly = threadReadOnly(view);
  const list = useRef<VirtualListHandle>(null);
  const root = useRef<HTMLDivElement>(null);
  usePromptFocus(root, focusPrompt, view.order);
  const [lit, setLit] = useState<ReadonlySet<string> | null>(null);
  const [seen, setSeen] = useState(0);
  const [atEnd, setAtEnd] = useState(true);
  const [restored] = useState(() => readTimelinePosition(read, view.read?.selection ?? null));
  const [positionRead, setPositionRead] = useState<'waiting' | 'done' | 'failed'>(restored?.atEnd === false && restored.item !== null && linked === null ? 'waiting' : 'done');
  const linkKey = linked === null ? null : `${threadId}\u0000${linked}`;
  const [linkIntent, setLinkIntent] = useState({ key: linkKey, dismissed: false });
  if (linkIntent.key !== linkKey) setLinkIntent({ key: linkKey, dismissed: false });
  const linkActive = linked !== null && (linkIntent.key !== linkKey || !linkIntent.dismissed);
  const [failedLink, setFailedLink] = useState<string | null>(null);
  const requestScope = `${view.read?.lifetime ?? 0}:${view.read?.epoch ?? 0}`;
  const [older, setOlder] = useState({ scope: requestScope, loading: false, failed: false });
  const loadingOlder = older.scope === requestScope && older.loading;
  const olderFailed = older.scope === requestScope && older.failed;
  const visible = useRef<ReadonlySet<string>>(new Set());
  const sideRevealed = useRef<{ threadId: string; request: number } | null>(null);

  const rows = useMemo<Row[]>(
    () => buildRows(view, { detail: preferences.detail, hideHistory: preferences.hideHistory, foldOpened: ui.folds, meId }),
    // The rows depend on these parts of the view and on nothing else.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [view.order, view.items, view.turns, view.thread, view.read, view.openedReadSeq, view.openedSeq, preferences.detail, preferences.hideHistory, ui.folds, meId],
  );
  const recent = useMemo(() => recentTurns(view.turns), [view.turns]);
  if (positionRead === 'waiting' && restored?.item !== null && restored?.item !== undefined && view.items.has(restored.item)) setPositionRead('done');
  useEffect(() => {
    if (positionRead !== 'waiting' || view.loaded === 'none' || restored?.item === null || restored?.item === undefined) return;
    if (view.items.has(restored.item)) return;
    const captured = captureThreadRead(read);
    const controller = new AbortController();
    fetchItems(read, { around: restored.item }, undefined, controller.signal).then((reply) => {
      if (!isCurrentThreadRead(captured) || controller.signal.aborted) return;
      threadWriter.window(read, reply.items, reply.has_older, reply.has_newer);
      setPositionRead('done');
    }).catch(() => { if (isCurrentThreadRead(captured) && !controller.signal.aborted) setPositionRead('failed'); });
    return () => controller.abort();
  }, [positionRead, view.loaded, view.items, restored, read]);
  const context = useMemo<TimelineContext>(
    () => ({
      threadId,
      threadName: thread?.name ?? '',
      detail: preferences.detail,
      recent,
      layout: preferences.layout,
      setLayout: preferences.setLayout,
      meId,
      canInteract: !readOnly && holds(thread?.access, 'thread.interact'),
      canManage: !readOnly && holds(thread?.access, 'thread.manage'),
      readOnly,
      onVersionSelect,
      live: socket.phase === 'ready',
      lit,
      selection: readOnly ? NOT_PICKING : selection,
    }),
    [threadId, thread, preferences.detail, preferences.layout, preferences.setLayout, recent, meId, socket.phase, lit, selection, readOnly, onVersionSelect],
  );

  // New rows below the reader's place: counted since they were last at the end.
  if (atEnd && seen !== rows.length) setSeen(rows.length);
  const away = atEnd ? 0 : Math.max(0, rows.length - seen);

  // Older items, read when the reader comes to the top.
  const loadOlder = useCallback(() => {
    if (!view.hasOlder || loadingOlder || view.order.length === 0) return;
    const first = view.items.get(view.order[0] ?? '');
    if (first === undefined) return;
    const captured = captureThreadRead(read);
    setOlder({ scope: requestScope, loading: true, failed: false });
    fetchItems(read, { before: first.seq_created, stream: 'main' })
      .then((reply) => threadWriter.older(read, reply.items, reply.has_older))
      .catch(() => { if (isCurrentThreadRead(captured)) setOlder({ scope: requestScope, loading: false, failed: true }); })
      .finally(() => setOlder((held) => held.scope === requestScope ? { ...held, loading: false } : held));
  }, [view.hasOlder, view.order, view.items, loadingOlder, read, requestScope]);

  // A link names an item: it is brought into view, from the server if the page does not hold it.
  const bring = useCallback(
    (itemId: string, also: readonly string[] = [], highlight = true) => {
      const captured = captureThreadRead(read);
      const show = () => {
        // Rows are drawn after this render; the list finds the row by its key, which for an item is its id,
        // and for an item inside the folded work of a turn is the line that stands for that turn.
        requestAnimationFrame(() => {
          if (!isCurrentThreadRead(captured)) return;
          const held = threadViewOf(read).items.get(itemId);
          const folded = held?.turn_id === null || held === undefined ? null : `fold:${held.turn_id}`;
          if (list.current?.scrollToKey(itemId, 'center') === true || (folded !== null && list.current?.scrollToKey(folded, 'center') === true)) {
            if (highlight) {
              const lighted: ReadonlySet<string> = new Set([itemId, ...also]);
              setLit(lighted);
              setTimeout(() => setLit((current) => (current === lighted ? null : current)), LIT_MS);
            }
          }
        });
      };
      if (threadViewOf(read).items.has(itemId)) {
        show();
        return;
      }
      fetchItems(read, { around: itemId })
        .then((reply) => {
          if (!isCurrentThreadRead(captured)) return;
          threadWriter.window(read, reply.items, reply.has_older, reply.has_newer);
          show();
        })
        .catch(() => { if (isCurrentThreadRead(captured)) setFailedLink(itemId); });
    },
    [read],
  );
  const jump = useCallback(() => {
    // The end of a historical window is not the conversation's newest end. Once
    // the reader leaves a permalink, its old target must not hide the fresh tail.
    setLinkIntent({ key: linkKey, dismissed: true });
    setLit(null);
    if (view.loaded === 'window' && view.hasNewer) reloadThread(read);
    list.current?.scrollToEnd();
  }, [linkKey, read, view.loaded, view.hasNewer]);
  useImperativeHandle(ref, () => ({ scrollToItem: bring, scrollToEnd: jump, visibleItemIds: () => visible.current }), [bring, jump]);
  useEffect(() => {
    if (linked !== null && linkActive && view.loaded !== 'none') bring(linked, alsoLinked ?? [], highlightLink);
    // Only a new link brings its item; the conversation growing does not.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linked, linkActive, view.loaded === 'none', highlightLink]);

  useEffect(() => {
    const focus = ui.sideFocus;
    if (!shown || focus === undefined || (sideRevealed.current?.threadId === threadId && sideRevealed.current.request === focus.request)) return;
    const marker = view.order.find((id) => {
      const held = view.items.get(id);
      return held?.kind === 'marker' && held.payload.marker === 'side_started' && held.payload.data.side.id === focus.id;
    });
    if (marker === undefined) return;
    sideRevealed.current = { threadId, request: focus.request };
    bring(marker);
  }, [ui.sideFocus, shown, threadId, view.order, view.items, bring]);

  // The newest item is marked read once it has been on screen, with the tab in front, for two seconds.
  const newest = view.order.length === 0 ? 0 : Math.max(view.thread?.seq ?? 0, view.items.get(view.order[view.order.length - 1] ?? '')?.seq_updated ?? 0);
  useEffect(() => {
    if (!shown || !atEnd || newest === 0 || view.loaded === 'window') return;
    const known = view.thread?.my_last_read_seq ?? 0;
    if (newest <= known) return;
    const timer = setTimeout(() => {
      if (document.visibilityState === 'visible') void markRead(read, newest).catch(() => undefined);
    }, READ_AFTER_MS);
    return () => clearTimeout(timer);
  }, [shown, atEnd, newest, read, view.loaded, view.thread?.my_last_read_seq]);

  // `End` jumps to the latest.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'End' || event.ctrlKey || event.metaKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target === null || root.current?.contains(target) !== true) return;
      if (target.isContentEditable || ['INPUT', 'TEXTAREA', 'SELECT'].includes(target.tagName)) return;
      if (target.closest('[role="log"]') === null) return;
      jump();
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [jump]);

  const renderRow = (index: number): ReactNode => {
    const row = rows[index];
    if (row === undefined) return null;
    return (
      <div className="mx-auto w-full max-w-(--reading-width) px-4" style={{ paddingTop: row.kind === 'footer' ? 0 : 'calc(var(--message-gap) / 2)', paddingBottom: 'calc(var(--message-gap) / 2)' }}>
        <RowView row={row} />
      </div>
    );
  };

  // A link to an item that is not held: nothing of the newest part is shown while the stretch around the item is read.
  const waitingForLink = linked !== null && linkActive && failedLink !== linked && !view.items.has(linked);
  if (view.loaded === 'none' || waitingForLink || positionRead === 'waiting') {
    if (view.closed !== null) return null;
    if (view.error !== null) return <ErrorState className="h-full" error={view.error} onRetry={() => reloadThread(read)} />;
    return <Skeletons />;
  }

  if (rows.length === 0) {
    return (
      <div className="flex h-full items-center-safe justify-center overflow-y-auto">
        <EmptyState compact={compact}
          icon={<Bot aria-hidden />}
          title={t(readOnly ? 'messageVersions.empty' : 'timeline.empty.title')}
          description={readOnly ? undefined : t('timeline.empty.about')}
          action={readOnly ? undefined : empty}
        />
      </div>
    );
  }

  return (
    <TimelineContextValue.Provider value={context}>
      <div ref={root} className="relative flex h-full min-h-0 flex-col">
        {(loadingOlder || olderFailed) && (
          <div className="absolute top-2 left-1/2 z-(--layer-sticky) -translate-x-1/2">
            {loadingOlder ? (
              <span role="status" className="flex items-center gap-2 rounded-full border border-border bg-surface-raised px-3 py-1 text-xs text-text-muted shadow-sm">
                <LoaderCircle className="animate-spin" aria-hidden /> {t('timeline.loadingEarlier')}
              </span>
            ) : (
              <StateLine role="alert" tone="danger" compact className="shadow-sm" message={t('timeline.earlierFailed')}
                action={<Button size="sm" variant="ghost" onClick={loadOlder}>{t('common.tryAgain')}</Button>} />
            )}
          </div>
        )}
        <VirtualList
          ref={list}
          label={t('timeline.label')}
          className="h-full min-h-0 overflow-y-auto"
          count={rows.length}
          startAtEnd={!linkActive || failedLink === linked}
          initialPosition={linked === null && positionRead === 'done' ? restored : null}
          keyOf={(index) => rows[index]?.key ?? String(index)}
          estimate={(index) => (rows[index] === undefined ? 48 : estimateRow(rows[index], view.items))}
          render={renderRow}
          onNearStart={loadOlder}
          onAtEnd={setAtEnd}
          onPosition={(position) => {
            const row = rows.find((row) => row.key === position.key);
            const item = row?.kind === 'item' || row?.kind === 'side' ? row.id : row?.kind === 'fold' || row?.kind === 'markers' ? row.ids[0] ?? null : null;
            keepTimelinePosition(read, view.read?.selection ?? null, { ...position, item });
          }}
          onRange={(first, last) => {
            const ids = new Set<string>();
            for (let at = first; at <= last; at += 1) {
              const row = rows[at];
              if (row?.kind === 'item' || row?.kind === 'side') ids.add(row.id);
            }
            visible.current = ids;
            onVisible?.(ids);
          }}
        />
        {(!atEnd || view.loaded === 'window' && view.hasNewer) && (
          <div className="pointer-events-none absolute right-0 bottom-3 left-0 flex justify-center">
            <Button className="pointer-events-auto shadow-md" variant="secondary" size="sm" icon={<ArrowDown aria-hidden />} onClick={jump}>
              {view.loaded !== 'window' && away > 0 ? t('timeline.newBelow', { count: away }) : t('timeline.jump')}
            </Button>
          </div>
        )}
      </div>
    </TimelineContextValue.Provider>
  );
}

/** One item of the conversation, read from the store by its id so that a change to one item draws that row alone. */
const ItemRow = memo(function ItemRow({ id, head }: { id: string; head: boolean }) {
  const { threadId, inspection } = useTimelineContext();
  const { t } = useTranslation();
  const item = useThreadPart(threadId, (view) => view.items.get(id));
  if (item === undefined) return null;
  switch (item.kind) {
    case 'user_message':
      return <UserMessage item={item} />;
    case 'agent_message':
      return <AgentMessage item={item} head={head} />;
    case 'reasoning':
      return <Reasoning item={item} />;
    case 'plan':
      return <Plan item={item} />;
    case 'tool_call':
      return <ToolCallRow threadId={threadId} id={id} />;
    case 'approval':
      return inspection === undefined ? <ApprovalCard item={item} /> : <InlineAlert tone="info">{t('trace.retainedApproval', { id: item.payload.approval_id })}</InlineAlert>;
    case 'announcement':
      return <AnnouncementCard item={item} />;
    case 'marker':
      return <MarkerLine item={item} />;
    case 'error':
      return <ErrorRow item={item} />;
  }
});

function MarkersRow({ ids }: { ids: readonly string[] }) {
  const { threadId } = useTimelineContext();
  const first = useThreadPart(threadId, (view) => view.items.get(ids[0] ?? ''));
  return first?.kind === 'marker' ? <MarkerLine item={first} count={ids.length} /> : null;
}

function NewLineRow() {
  const { threadId } = useTimelineContext();
  return <NewLine threadId={threadId} />;
}

/** One row of the conversation. */
function RowView({ row }: { row: Row }) {
  switch (row.kind) {
    case 'item':
      return <ItemRow id={row.id} head={row.head === true} />;
    case 'fold':
      return <WorkFold turnId={row.turnId} ids={row.ids} open={row.open} />;
    case 'markers':
      return <MarkersRow ids={row.ids} />;
    case 'side':
      return <SideThreadLaneRow id={row.id} {...(row.closedId === undefined ? {} : { closedId: row.closedId })} {...(row.sourceTurn === undefined ? {} : { sourceTurn: row.sourceTurn })} />;
    case 'footer':
      return <TurnFooter turnId={row.turnId} inputItemId={row.inputItemId} />;
    case 'divider':
      return <NewLineRow />;
    case 'status':
      return <StatusLine />;
    case 'before':
      return <BeforeOrchHeading />;
  }
}
