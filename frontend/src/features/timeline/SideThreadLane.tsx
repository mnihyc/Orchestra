/** A side thread beside the conversation: its own short exchange with the same agent, named after its thread, kept by a fork or closed.
 * The aside's feed and server draft are mounted only while its lane or panel is shown. Shared lane
 * displays keep messages, native work and answers compact; its queue and Stop belong to the aside.
 */
import { useStore } from 'zustand';
import { ChevronDown, ChevronRight, MessageSquareDashed, SendHorizontal } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { ApiError, client } from '@/api/client';
import { fetchItems, OLDER_ITEMS } from '@/api/threads';
import { captureThreadRead, isCurrentThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { ThreadReadProvider, useThreadReadRef } from '@/realtime/threadReadContext';
import { defaultThreadRead, resultThreadRead, sameThreadRead, type ThreadReadRef } from '@/realtime/threadScope';
import { allTabs, setTabTarget } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { ContinuationNotice } from '@/features/thread/ContinuationNotice';
import { openChange } from '@/features/change/target';
import { DraftMark } from '@/features/composer/DraftMark';
import { AttachmentChips } from '@/features/composer/Attachments';
import { composerWriter, submitInput, useAttachmentParts, useBox, useDraftSync, useSending } from '@/features/composer/Draft';
import { QueueTray } from '@/features/composer/QueueTray';
import { StopControl, useStop } from '@/features/composer/StopControl';
import { useCloseSide } from '@/features/side-threads/ask';
import { threadCapabilities } from '@/features/thread/cached';
import { NOT_PICKING, type PromptFocus, type SelectPromptVersion, type TimelineContext, useTimelineContext } from '@/features/timeline/context';
import { usePromptFocus } from '@/features/timeline/usePromptFocus';
import { useTimelineWindowKey } from '@/features/timeline/windowKey';
import { ConversationLane } from '@/features/timeline/Lane';
import { recentTurns, queuedInputs, sendingInTray } from '@/features/timeline/rows';
import { sideLaneKey, timelineUi, useThreadUi } from '@/features/timeline/state';
import { revealView } from '@/features/workbench/openView';
import { useViewShown, type ViewProps } from '@/features/workbench/views';
import type { ContentPart, MarkerSideClosed, NodeRef, Thread, UserRef } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { isComposing } from '@/lib/shortcuts';
import { forgetThreadRead, reloadThread, useSocketStatus, useThreadFeed } from '@/realtime/live';
import { runningTurn, threadReadOnly, threadViewOf, threadWriter, useThreadPart, useThreadView } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { ScrollArea } from '@/ui/ScrollArea';
import { SkeletonRows } from '@/ui/Skeleton';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { Textarea } from '@/ui/Textarea';
import { TimeOfDay } from '@/ui/TimeOfDay';
import { Tooltip } from '@/ui/Tooltip';

function SideComposer({ thread, live }: { thread: Thread; live: boolean }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(thread.id);
  const { text, setText, refusal } = useBox(thread.id);
  const parts = useAttachmentParts(thread.id);
  const empty = text.trim() === '' && parts.length === 0;
  const draft = useDraftSync(thread.id, true, 'queue');
  const turn = useThreadPart(thread.id, runningTurn);
  const stop = useStop(thread.id, turn?.id ?? null, live);
  const send = () => {
    if (!live || empty || thread.archived || thread.lock !== null) return;
    const content: ContentPart[] = [...(text.trim() === '' ? [] : [{ type: 'text' as const, text }]), ...parts];
    setText('');
    composerWriter.setParts(thread.id, []);
    submitInput(read, content, 'queue');
  };
  return <div className="flex min-w-0 flex-col gap-1" data-side-composer={thread.id}>
    <AttachmentChips parts={parts} onRemove={(index) => composerWriter.setParts(thread.id, parts.filter((_, at) => at !== index))} />
    <div className="flex min-w-0 flex-wrap items-end gap-1">
      <Textarea label={t('side.lane.box', { name: thread.name })} hideLabel rows={1} placeholder={t('side.lane.placeholder')}
        className="resize-none text-sm" fieldClassName="min-w-0 flex-1" value={text} onChange={(event) => setText(event.target.value)}
        aria-keyshortcuts="Enter Control+Enter Meta+Enter" onKeyDown={(event) => {
          if (event.key === 'Escape' && text.trim() === '' && turn !== null && !isComposing(event.nativeEvent)) {
            event.preventDefault(); event.stopPropagation(); stop.escape(); return;
          }
          if (event.key === 'Enter' && !event.shiftKey && !event.altKey && !isComposing(event.nativeEvent)) {
            event.preventDefault(); event.stopPropagation(); send();
          }
        }} />
      <DraftMark status={draft} />
      {turn !== null && <StopControl stop={stop} disabled={!live} />}
      <Button size="sm" variant="primary" icon={<SendHorizontal aria-hidden />} disabled={!live || empty || thread.archived || thread.lock !== null} onClick={send}>
        {t(turn === null ? 'side.lane.send' : 'side.lane.queue')}
      </Button>
    </div>
    {!live && <p role="status" className="text-xs text-text-muted">{t('composer.reason.offline')}</p>}
    {refusal !== undefined && refusal !== null && <ErrorAlert error={refusal} />}
  </div>;
}

interface SideLocation { continuationId: string | null; promptId: string | undefined; itemId: string | null }
const ACTIVE_SIDE: SideLocation = { continuationId: null, promptId: undefined, itemId: null };
interface SideFocus { read: ThreadReadRef; reader: number | undefined; request: PromptFocus }
interface SideConversationProps {
  parentId: string; sideId: string; panel?: boolean; by?: UserRef; at?: string; readOwner?: string;
  location?: SideLocation; onLocation?: (next: SideLocation) => void;
}
interface SideRequest {
  scope: AbortController | null; controller: AbortController; loading: boolean; error: unknown;
}
interface SideAnchorRequest extends SideRequest { itemId: string; retry: number }

function SideConversation(props: SideConversationProps) {
  const parentRead = useThreadReadRef(props.parentId);
  const owner = props.readOwner ?? (parentRead.key.kind === 'result' ? parentRead.key.owner : null);
  const read = useMemo(() => owner === null ? defaultThreadRead(props.sideId) : resultThreadRead(props.sideId, owner), [owner, props.sideId]);
  const [local, setLocal] = useState<SideLocation>(ACTIVE_SIDE);
  const [focus, setFocus] = useState<SideFocus | null>(null);
  const { panel, readOwner, parentId, sideId, onLocation } = props;
  const location = props.location ?? local;
  const selectVersion = useCallback<SelectPromptVersion>((id, anchor) => {
    const next: SideLocation = { continuationId: id, promptId: anchor?.inputId, itemId: anchor?.itemId ?? null };
    if (panel) {
      if (readOwner === undefined) return;
      const tab = allTabs(useWorkbench.getState().layout).find((one) => one.id === readOwner);
      if (tab?.of?.thread !== parentId || tab.of.side !== sideId) return;
      useWorkbench.getState().change((layout) => {
        const current = allTabs(layout).find((one) => one.id === readOwner);
        if (current?.of?.thread !== parentId || current.of.side !== sideId) return layout;
        const of: Record<string, string> = { ...current.of, continuation: id ?? '' };
        delete of.prompt; delete of.item;
        if (next.promptId !== undefined) of.prompt = next.promptId;
        if (next.itemId !== null) of.item = next.itemId;
        return setTabTarget(layout, readOwner, of);
      });
    } else if (onLocation !== undefined) onLocation(next);
    else setLocal(next);
    // The locator chooses only this read. Native execution and the parent's route stay unchanged.
    selectThreadContinuation(read, id);
    setFocus((held) => anchor?.focus === true ? { read, reader: client.session.getState().readerVersion,
      request: { groupId: anchor.groupId, request: (held?.request.request ?? 0) + 1 } } : null);
  }, [onLocation, panel, parentId, read, readOwner, sideId]);
  useLayoutEffect(() => {
    const held = threadViewOf(read).read;
    // Preserve the legacy active-only renderer without inventing an unopened scope for it.
    if (location.continuationId !== null || held !== null && held.selection !== null) {
      selectThreadContinuation(read, location.continuationId);
    }
  }, [location.continuationId, read]);
  return <ThreadReadProvider read={read}><SideConversationBody {...props} location={location} read={read} parentRead={parentRead}
    onVersionSelect={selectVersion} focus={focus} /></ThreadReadProvider>;
}

function SideConversationBody({ parentId, sideId, panel = false, by, at, readOwner, read, parentRead, location, onVersionSelect, focus }: SideConversationProps & {
  read: ThreadReadRef; parentRead: ThreadReadRef; location: SideLocation; onVersionSelect: SelectPromptVersion; focus: SideFocus | null;
}) {
  const { t } = useTranslation();
  const parent = useTimelineContext();
  useThreadFeed(read);
  const view = useThreadView(read);
  const windowKey = useTimelineWindowKey(view);
  const readOnly = threadReadOnly(view);
  const scope = view.read?.controller ?? null;
  const ready = view.read?.ready === true && view.loaded !== 'none' && view.closed === null;
  useEffect(() => {
    const owner = readOwner ?? (parentRead.key.kind === 'result' ? parentRead.key.owner : null);
    if (owner === null) return;
    return () => { queueMicrotask(() => {
      const tab = allTabs(useWorkbench.getState().layout).find((one) => one.id === owner);
      const retained = tab !== undefined && (readOwner === undefined ? tab.of?.id === parentId : tab.of?.side === sideId);
      if (!retained) { forgetThreadRead(read); timelineUi.forget(read); }
    }); };
  }, [parentId, parentRead, read, readOwner, sideId]);
  const meId = useStore(client.session, (state) => state.userId);
  const live = useSocketStatus().phase === 'ready';
  const sending = useSending(sideId);
  const close = useCloseSide(parentId);
  const itemsRoot = useRef<HTMLDivElement>(null);
  const [history, setHistory] = useState<SideRequest | null>(null);
  const [anchor, setAnchor] = useState<SideAnchorRequest | null>(null);
  const [anchorRetry, setAnchorRetry] = useState(0);
  const historyRead = useRef<SideRequest | null>(null);
  const anchorRead = useRef<SideRequest | null>(null);
  useEffect(() => () => {
    // A retired read cannot cancel a newer owner's page, even if its cleanup runs later.
    for (const held of [historyRead.current, anchorRead.current]) if (held?.scope === scope) held.controller.abort();
  }, [read, scope]);
  const historyNow = history?.scope === scope && !history.controller.signal.aborted ? history : null;
  const reading = historyNow?.loading === true;
  const readError = historyNow?.error ?? null;
  const nativeAnchor = location.promptId === undefined ? null
    : view.viewedPrompt?.input_id === location.promptId ? view.viewedPrompt.item_id ?? null : null;
  const heldAnchor = location.promptId === undefined ? null : [...view.items.values()]
    .find((item) => item.kind === 'user_message' && item.payload.input_id === location.promptId)?.id ?? null;
  const itemId = location.itemId ?? nativeAnchor ?? heldAnchor;
  const anchorNow = anchor?.scope === scope && anchor.itemId === itemId && anchor.retry === anchorRetry
    && !anchor.controller.signal.aborted ? anchor : null;
  const anchorError = anchorNow?.error ?? null;
  // The read intent already says that this delivered row is missing; no synchronous effect state is needed.
  const anchoring = ready && itemId !== null && !view.items.has(itemId) && anchorError === null;
  useEffect(() => {
    if (itemId === null || !ready || threadViewOf(read).items.has(itemId)) return;
    const captured = captureThreadRead(read);
    const request: SideAnchorRequest = { scope, controller: new AbortController(), loading: true, error: null, itemId, retry: anchorRetry };
    anchorRead.current = request;
    void fetchItems(read, { around: itemId, stream: 'main' }, OLDER_ITEMS, request.controller.signal).then((page) => {
      if (!isCurrentThreadRead(captured, request.controller.signal)) return;
      if (!page.items.some((item) => item.id === itemId)) throw new Error('The selected message is no longer available in this history.');
      threadWriter.window(read, page.items, page.has_older, page.has_newer, captured);
    }).catch((error: unknown) => {
      if (isCurrentThreadRead(captured, request.controller.signal)) setAnchor({ ...request, loading: false, error });
    }).finally(() => {
      if (anchorRead.current === request) anchorRead.current = null;
    });
    return () => { request.controller.abort(); };
  }, [anchorRetry, itemId, read, ready, scope]);
  const brought = useRef<{ scope: AbortController | null; itemId: string } | null>(null);
  useEffect(() => {
    if (!ready || itemId === null || brought.current?.scope === scope && brought.current.itemId === itemId) return;
    const element = [...(itemsRoot.current?.querySelectorAll<HTMLElement>('[data-item]') ?? [])].find((one) => one.dataset.item === itemId);
    if (element === undefined) return;
    element.scrollIntoView({ block: 'nearest' });
    brought.current = { scope, itemId };
  }, [itemId, ready, scope, view.order]);
  const focusPrompt = ready && focus !== null && sameThreadRead(focus.read, read) && focus.reader === view.read?.reader ? focus.request : null;
  usePromptFocus(itemsRoot, focusPrompt, view.order);
  const earlier = async () => {
    const held = threadViewOf(read);
    const first = held.items.get(held.order[0] ?? '');
    if (first === undefined || reading || anchoring) return;
    const captured = captureThreadRead(read);
    const request: SideRequest = { scope: captured.scope.controller, controller: new AbortController(), loading: true, error: null };
    historyRead.current?.controller.abort(); historyRead.current = request; setHistory(request);
    try {
      const page = await fetchItems(read, { before: first.seq_created, stream: 'main' }, OLDER_ITEMS, request.controller.signal);
      if (isCurrentThreadRead(captured, request.controller.signal)) threadWriter.older(read, page.items, page.has_older, captured);
    } catch (error) {
      if (isCurrentThreadRead(captured, request.controller.signal)) setHistory({ ...request, loading: false, error });
    } finally {
      if (historyRead.current === request) historyRead.current = null;
      if (isCurrentThreadRead(captured, request.controller.signal)) setHistory((current) => current?.controller === request.controller ? { ...current, loading: false } : current);
    }
  };
  const jump = () => { onVersionSelect(location.continuationId); reloadThread(read); };
  const thread = view.thread;
  const recent = useMemo(() => recentTurns(view.turns), [view.turns]);
  const context = useMemo<TimelineContext>(() => ({ threadId: sideId, threadName: thread?.name ?? '', detail: 'steps', recent, layout: 'one', setLayout: () => undefined,
    meId, canInteract: !readOnly && holds(thread?.access, 'thread.interact'), canManage: !readOnly && holds(thread?.access, 'thread.manage'), readOnly, live, lit: null, selection: NOT_PICKING,
    onVersionSelect, laneThreads: [...(parent.laneThreads ?? []), sideId] }), [sideId, thread?.name, thread?.access, recent, meId, live, parent.laneThreads, readOnly, onVersionSelect]);
  const queued = useThreadPart(sideId, useShallow(queuedInputs));
  const pending = useMemo(() => sendingInTray(view, sending), [view, sending]);
  if (view.closed === 'deleted') return <p className="text-sm text-text-muted">{t('side.lane.closedUnknown', { name: thread?.name ?? t('side.lane.unnamed') })}</p>;
  if (view.closed !== null) return <p className="text-sm text-text-muted">{t('side.lane.unavailable')}</p>;
  if (view.error instanceof ApiError && (view.error.status === 403 || view.error.status === 404)) return <ErrorState compact error={view.error} />;
  if (readError instanceof ApiError && (readError.status === 403 || readError.status === 404)) return <ErrorState compact error={readError} />;
  if (anchorError instanceof ApiError && (anchorError.status === 403 || anchorError.status === 404)) return <ErrorState compact error={anchorError} />;
  if (view.error !== null && thread === null) return <ErrorState compact error={view.error} onRetry={() => reloadThread(read)} />;
  if (thread === null) return <SkeletonRows rows={2} />;
  if (thread.side_of?.id !== parentId) return <p className="text-sm text-text-muted">{t('side.lane.unavailable')}</p>;
  const mayDrive = context.canInteract && !thread.archived;
  const mayClose = (meId === thread.owner.id || context.canManage || parent.canManage) && live;
  return <ConversationLane kind="side" identity={sideId} label={t('side.lane.label', { name: thread.name })} activityLabel={t('side.lane.conversation')}
    itemsRef={itemsRoot} context={context} ids={view.order} loading={view.loaded === 'none'} empty={t('side.lane.empty')} agentHeads
    header={<>
      {panel && <><ThreadStateIcon display={thread.display} /><span className="min-w-0 wrap-anywhere font-medium">{thread.name}</span></>}
      {!panel && by !== undefined && <span className="text-xs text-text-muted">{t('timeline.marker.sideStarted', { by: by.display_name })}</span>}
      {at !== undefined && <TimeOfDay at={at} className="text-xs text-text-muted" />}
      {!panel && <Button size="sm" variant="ghost" className="ml-auto" onClick={() => {
        const target: Record<string, string> = { thread: parentId, side: sideId };
        if (location.continuationId !== null) target.continuation = location.continuationId;
        if (location.promptId !== undefined) target.prompt = location.promptId;
        if (itemId !== null) target.item = itemId;
        timelineUi.setStep(parentRead, sideLaneKey(sideId), false); revealView('side-thread', target);
      }}>{t('side.lane.openPanel')}</Button>}
      {threadCapabilities(thread)?.fork === true && <Button size="sm" variant="ghost" disabled={readOnly || !live || thread.lock !== null}
        title={readOnly ? t('messageVersions.activeActions') : undefined} onClick={() => openChange({ kind: 'thread', id: sideId, name: thread.name }, { kind: 'fork' })}>{t('side.keep')}</Button>}
      {(meId === thread.owner.id || context.canManage || parent.canManage) && <Button size="sm" variant="ghost" pending={close.isPending} disabled={!mayClose}
        onClick={() => close.mutate(sideId)}>{t('side.close')}</Button>}
    </>}
    intro={<p className="text-xs text-text-muted">{thread.side_reading === null ? t('side.lane.modeUnknown') : t('side.ask.mode', { mode: t(`side.reading.${thread.side_reading}`) })}</p>}
    history={<>{view.hasOlder && <Button size="sm" variant="ghost" pending={reading} disabled={anchoring} onClick={() => void earlier()}>{t('side.lane.older')}</Button>}
      {view.loaded === 'window' && view.hasNewer && <Button size="sm" variant="ghost" onClick={jump}>{t('timeline.jump')}</Button>}
      {readError !== null && <ErrorState compact error={readError} onRetry={() => void earlier()} />}
      {anchorError !== null && <ErrorState compact error={anchorError} onRetry={() => setAnchorRetry((value) => value + 1)} />}</>}
    footer={<>
      {close.error !== null && <ErrorAlert error={close.error} />}
      <ContinuationNotice key={windowKey} threadId={sideId} view={view} onSelect={onVersionSelect} />
      <QueueTray threadId={sideId} queued={queued} sending={pending} readOnly={readOnly} />
      {mayDrive ? <SideComposer thread={thread} live={live} /> : <p role="status" className="text-xs text-text-muted">{t('side.lane.readOnly')}</p>}
    </>}
  />;
}

/** A start marker stays at its source turn and closes to one historical row. Folded asides mount no feed. */
export function SideLaneSlot({ side, by, at, closed }: { side: NodeRef; by?: UserRef; at?: string; closed?: MarkerSideClosed }) {
  const { t } = useTranslation();
  const parent = useTimelineContext();
  const shown = useViewShown();
  const parentRead = useThreadReadRef(parent.threadId);
  const [location, setLocation] = useState<SideLocation>(ACTIVE_SIDE);
  const ui = useThreadUi(parent.threadId);
  const held = useThreadPart(side.id, (view) => view.thread);
  const ended = useThreadPart(side.id, (view) => view.closed);
  const name = side.redacted ? t('side.lane.unnamed') : parent.inspection === undefined && held?.side_of?.id === parent.threadId ? held.name : side.name ?? t('side.lane.unnamed');
  const open = ui.steps.get(sideLaneKey(side.id)) ?? false;
  if (!shown) return null;
  if (closed !== undefined) return <div data-side-closed={side.id} className="flex items-center gap-2 py-1 text-sm text-text-muted"><MessageSquareDashed aria-hidden />
    <span className="wrap-anywhere">{t('side.lane.closed', { name, count: closed.turns })}</span>
  </div>;
  if (ended === 'deleted' && parent.inspection === undefined) return <p data-side-closed={side.id} className="py-1 text-sm text-text-muted">{t('side.lane.closedUnknown', { name })}</p>;
  if (parent.inspection !== undefined || parent.laneThreads?.includes(side.id) === true || side.redacted) return <p className="text-xs text-text-muted">{t('side.lane.retained')}</p>;
  return <div className="flex min-w-0 flex-col gap-1" data-side-slot={side.id}>
    <Tooltip content={name}><button type="button" aria-expanded={open} onClick={() => timelineUi.setStep(parentRead, sideLaneKey(side.id), !open)}
      className="focus-inset flex min-h-8 w-full items-center gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-bg-muted">
      {open ? <ChevronDown aria-hidden /> : <ChevronRight aria-hidden />}<ThreadStateIcon display={held?.side_of?.id === parent.threadId ? held.display : side.thread_display} />
      <span className="min-w-0 flex-1 truncate font-medium">{name}</span>
    </button></Tooltip>
    {open && <SideConversation key={side.id} parentId={parent.threadId} sideId={side.id} location={location} onLocation={setLocation} {...(by === undefined ? {} : { by })} {...(at === undefined ? {} : { at })} />}
  </div>;
}

/** A native side marker carries the source boundary and closed summary without another lookup. */
export function SideThreadLaneRow({ id, closedId, sourceTurn }: { id: string; closedId?: string; sourceTurn?: number }) {
  const { t } = useTranslation();
  const { threadId } = useTimelineContext();
  const item = useThreadPart(threadId, (view) => view.items.get(id));
  const ended = useThreadPart(threadId, (view) => closedId === undefined ? undefined : view.items.get(closedId));
  const closed = ended?.kind === 'marker' && ended.payload.marker === 'side_closed' ? ended.payload.data : undefined;
  if (item?.kind !== 'marker') return null;
  if (item.payload.marker === 'side_closed') return <SideLaneSlot side={item.payload.data.side} closed={item.payload.data} />;
  if (item.payload.marker !== 'side_started') return null;
  return <>
    {sourceTurn !== undefined && <p className="text-xs text-text-muted">{sourceTurn === 0 ? t('side.lane.beforeFirst') : t('side.lane.fromTurn', { number: sourceTurn })}</p>}
    <SideLaneSlot side={item.payload.data.side} by={item.payload.data.by} at={item.created_at} {...(closed?.side.id === item.payload.data.side.id ? { closed } : {})} />
  </>;
}

/** The same aside and inline composer in the existing Conversation workbench container. */
export function SideThreadPanel({ tab }: ViewProps) {
  const { t } = useTranslation();
  const shown = useViewShown();
  const parentId = tab.of?.thread;
  const sideId = tab.of?.side;
  return !shown || parentId === undefined || sideId === undefined ? null : <ScrollArea role="region" aria-label={t('side.panelTitle')} tabIndex={0}
    className="focus-inset h-full p-3" data-side-panel={sideId}><SideConversation key={sideId} parentId={parentId} sideId={sideId} readOwner={tab.id} panel location={{ continuationId: tab.of?.continuation === '' ? null : tab.of?.continuation ?? null, promptId: tab.of?.prompt, itemId: tab.of?.item ?? null }} /></ScrollArea>;
}
