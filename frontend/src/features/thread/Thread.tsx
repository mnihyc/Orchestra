import { threadIconFacts } from '@/ui/ThreadState';
/** The thread as a screen of the workbench: the conversation with its composer, and its other views in the arrangement chosen.
 *
 * The screen of a thread has three parts and only the middle one scrolls. The head is one row (and Find's bar under
 * it) that stays where it is. The conversation scrolls: the lines that tell of the thread, the summary, the README and
 * the conversation with the agent's work. The dock stays under it: what stands above the composer (why the thread
 * cannot go on, what holds it, approvals that wait, the queue, a plan) and the composer, disabled for somebody who
 * may only watch, or the line that the thread is archived.
 * The thread is followed live, so the screen is always the thread as it is now, and says when
 * it could not be brought up to date.
 */
import { ThreadSummaryInspector } from '@/features/summaries/ThreadSummaryInspector';
import { useMutation, useQuery } from '@tanstack/react-query';
import { Archive, Lock, MessageSquareText } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useViewInstance, useContentFocused } from '@/features/workbench/instance';
import { useViewShown } from '@/features/workbench/viewOpening';
import { ApiError, client } from '@/api/client';
import { unarchiveNode } from '@/api/nodes';
import { limitsQuery } from '@/api/threads';
import { selectThreadContinuation } from '@/api/threadRead';
import { viewedItemQuery, viewedItemsQuery } from '@/api/threadHistory';
import { ContinuationNotice } from '@/features/thread/ContinuationNotice';
import { useReadWhenOpened } from '@/features/inbox/Counters';
import { NoAccess } from '@/features/access/NoAccess';
import { readableNavigationRoot } from '@/features/navigation/currentOrg';
import { Composer } from '@/features/composer/Composer';
import { ThreadRunsLine } from '@/features/composer/Controls';
import { composerWriter, useSending } from '@/features/composer/Draft';
import { QueueTray } from '@/features/composer/QueueTray';
import { PlaceReadme } from '@/features/documents/Readme';
import { PersonThingProvider } from '@/features/people/personContext';
import { useSelection } from '@/features/forward/Selection';
import { LocalAddresses } from '@/features/ports/LocalAddresses';
import { BindingSummary } from '@/features/thread/BindingSummary';
import { ThreadGatewayStatus } from '@/features/gateways/ThreadGatewayStatus';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { threadSwitchRecovery } from '@/features/thread/switchRecovery';
import { ConnectionBanner } from '@/features/thread/ConnectionBanner';
import { SourcePreparation } from '@/features/source/Preparation';
import { ReviewTray } from '@/features/source/Review';
import { ThreadHeader } from '@/features/thread/Header';
import { AudienceBanner } from '@/features/thread/AudienceBanner';
import { ThreadNowProvider } from '@/features/thread/NowLine';
import { AgentSwitchContext } from '@/features/thread/agentSwitch';
import { FollowButton } from '@/features/thread/FollowButton';
import { useMessageKeys, useMessageNav } from '@/features/thread/messageMoves';
import { StickyPrompt } from '@/features/thread/MessageParts';
import { useReplyRoom } from '@/features/thread/replyRoom';
import { SentMessageBlock } from '@/features/thread/SentMessageBlock';
import { partitionWaiting } from '@/features/thread/sentMessages';
import { PendingAgentSchedules } from '@/features/schedules/Schedules';
import { PinnedApprovals } from '@/features/thread/PinnedApprovals';
import { isThreadForScreen, threadCapabilities } from '@/features/thread/cached';
import { GoalEditorHost } from '@/features/goal/GoalEditor';
import { GoalLine } from '@/features/goal/GoalLine';
import { LockBanner } from '@/features/thread/LockBanner';
import { PassiveStrip } from '@/features/thread/PassiveStrip';
import type { ItemSelection, PromptFocus, SelectPromptVersion } from '@/features/timeline/context';
import { FindInConversation } from '@/features/timeline/FindInConversation';
import { PlanLine } from '@/features/timeline/PlanLine';
import { StatusLine } from '@/features/timeline/StatusLine';
import { SubagentConversation } from '@/features/timeline/SubagentLane';
import { TimelineSelectionBar } from '@/features/timeline/SelectionBar';
import { type FollowState, Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import { useTimelineWindowKey } from '@/features/timeline/windowKey';
import { holds } from '@/lib/permissions';
import { refreshThread, reloadThread, useSocketStatus, useThreadFeed } from '@/realtime/live';
import { useReadAgain } from '@/lib/useReadAgain';
import { threadReadState, threadViewOf, useThreadView } from '@/realtime/stores/thread';
import { selectOrg } from '@/shell/org';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { LimitPrompt } from '@/ui/LimitPrompt';

function Frame({ children, className }: { children: React.ReactNode; className?: string }) {
  return <div className={cx('mx-auto flex w-full max-w-(--reading-width) shrink-0 flex-col gap-2 px-4 pb-3', className)}>{children}</div>;
}

function ThreadSkeleton() {
  return (
    <SkeletonGroup className="flex h-full flex-col">
      <div className="flex flex-col gap-2 border-b border-border px-4 py-3">
        <Skeleton className="h-6 w-1/3" />
        <Skeleton className="h-4 w-1/2" />
      </div>
      <div className="mx-auto flex w-full max-w-(--reading-width) flex-1 flex-col gap-6 px-4 py-6">
        {[0, 1, 2].map((row) => (
          <div key={row} className="flex gap-3">
            <Skeleton className="size-(--avatar-message) rounded-full" />
            <div className="flex flex-1 flex-col gap-2">
              <Skeleton className="h-3 w-(--skeleton-line)" />
              <Skeleton className="h-4 w-3/4" />
            </div>
          </div>
        ))}
      </div>
    </SkeletonGroup>
  );
}

export interface ThreadProps {
  threadId: string;
  /** An item the address names. */
  item: string | null;
  /** The other items the address names, lit with it: a link to a selection. */
  items?: readonly string[] | null;
  turn?: number;
  continuation?: string | null;
  onVersionSelect?: SelectPromptVersion | undefined;
  prompt?: string | undefined;
  focusPrompt?: PromptFocus | null | undefined;
  /** The step of a sub-agent whose conversation is shown in place of the main one: the address carries it. */
  agent?: string | null;
  /** Shows a sub-agent's conversation (its step) or the main one again (`null`), by changing the address. */
  onAgent?: ((step: string | null) => void) | undefined;
}

/** The items in view before the list has said which they are. */
const NOTHING_IN_VIEW: ReadonlySet<string> = new Set();

/** While the page shows a turn as under way, the thread is read again this often (while the live connection is up; every three seconds while it is not): a frame that never came would otherwise leave "Generating" on the screen until the page was loaded again. Small: one read of what changed since the last row held. */
const RUNNING_READ_AGAIN_MS = 5_000;

export function ThreadScreen({ threadId, item, items = null, turn, continuation = null, onVersionSelect, prompt, focusPrompt, agent = null, onAgent }: ThreadProps) {
  const { t } = useTranslation();
  const userId = useStore(client.session, (state) => state.userId);
  const read = useThreadReadRef(threadId);
  const instance = useViewInstance();
  const focusedContent = useContentFocused();
  const shown = useViewShown();
  // The whole screen is what Find answers for (its key reaches it from the composer too); the middle part is what scrolls.
  const frame = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState<HTMLDivElement | null>(null);
  const [spacer, setSpacer] = useState<HTMLDivElement | null>(null);
  // The lines that tell of the thread are drawn by the header, into this place at the top of the scrolling part.
  const [notesHost, setNotesHost] = useState<HTMLDivElement | null>(null);
  const timeline = useRef<TimelineHandle>(null);
  const childTimeline = useRef<TimelineHandle>(null);
  const [proceeding, setProceeding] = useState(false);
  useLayoutEffect(() => selectThreadContinuation(read, continuation), [read, continuation]);
  useThreadFeed(threadId);
  const view = useThreadView(threadId);
  const windowKey = useTimelineWindowKey(view);
  const selectedOptions = viewedItemQuery(read, agent ?? '');
  const selectedParent = useQuery({ ...selectedOptions, enabled: agent !== null && !view.items.has(agent) && view.thread !== null && view.closed === null && selectedOptions.enabled, retry: false });
  const reading = threadReadState(view);
  const promptItem = prompt === undefined ? null : (view.viewedPrompt?.input_id === prompt ? view.viewedPrompt.item_id : null)
    ?? [...view.items.values()].find((one) => one.kind === 'user_message' && one.payload.input_id === prompt)?.id ?? null;
  const linkedOptions = viewedItemsQuery(read, { turn: turn ?? 0 });
  const linkedTurn = useQuery({ ...linkedOptions, enabled: turn !== undefined && item === null && linkedOptions.enabled, retry: false });
  // Only the actually displayed conversation marks its notifications read; frame metadata does not.
  useReadWhenOpened(!reading.readOnly && view.closed === null && view.loaded !== 'none' ? threadId : null);
  const thread = isThreadForScreen(view.thread, threadId) ? view.thread : null;
  // A read that begins again (a live frame was lost, the connection came back) leaves the box where it stands and usable, so what is being typed keeps its place and its focus: removing it made the row of controls vanish and the page jump.
  const [boxWas, setBoxWas] = useState(false);
  const holdBox = boxWas && thread !== null && !thread.archived && reading.readiness === 'loading' && reading.selection === null
    && (reading.projection === null || reading.projection.viewed_continuation_id === reading.projection.active_continuation_id);
  const boxNow = holdBox || thread !== null && !reading.readOnly && !thread.archived;
  // Remembered from one render to the next; adjusted while rendering, as React asks for a value that follows others.
  if (boxWas !== boxNow) setBoxWas(boxNow);
  const navigationOrg = readableNavigationRoot(thread?.path);
  // The items being picked, to forward, quote or discuss: held here, where the conversation and the bar that stands in place of the box meet.
  const picking = useSelection<string>(view.order);
  const selection = useMemo<ItemSelection>(
    () => ({ active: picking.active, has: picking.has, toggle: picking.toggle, start: picking.start, pick: picking.pick }),
    [picking],
  );
  // X begins picking, from the item that has the focus or else the last one, wherever no text is being written.
  const { active: picked, start: startPicking } = picking;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (instance !== null && (!(event.target instanceof Node) || instance.root.current?.contains(event.target) !== true)) return;
      if (reading.readOnly || picked || event.defaultPrevented || event.altKey || event.ctrlKey || event.metaKey || (event.key !== 'x' && event.key !== 'X')) return;
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (target !== null && (target.isContentEditable || target.closest('input, textarea, select, [role="dialog"], [role="menu"]') !== null)) return;
      const focused = document.activeElement instanceof HTMLElement ? document.activeElement.closest('[data-item]')?.getAttribute('data-item') : null;
      const id = focused ?? view.order[view.order.length - 1] ?? null;
      if (id === null) return;
      event.preventDefault();
      startPicking(id);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [picked, startPicking, view.order, reading.readOnly, instance]);
  useScreenHeading({ title: view.closed === null ? thread?.name ?? t('thread.screen.title') : t('craft.threadUnavailable'), icon: MessageSquareText, thread: threadIconFacts(view.closed === null ? thread : undefined), nodeId: view.closed === null ? thread?.id : undefined, path: thread?.path, views: { ready: thread !== null && view.closed === null, workspace: thread?.environment !== null && thread?.environment_permissions.includes('files.view') === true, terminal: thread?.environment_permissions.includes('terminal.manage') === true, processes: thread?.environment_permissions.includes('terminal.view') === true } });
  useEffect(() => {
    if (focusedContent && navigationOrg !== null) selectOrg(userId, navigationOrg);
  }, [navigationOrg, userId, focusedContent]);

  // The status stays true without the live connection's help: while a turn is shown as under way the thread is read again, once more when the page is looked at again, and once when the connection comes back. A thread that is being opened again is left to that opening.
  const turnShown = thread !== null && (thread.state === 'running' || thread.state === 'starting' || thread.state === 'waiting_approval');
  const readNow = useCallback(() => { if (document.visibilityState === 'visible' && threadReadState(threadViewOf(read)).readiness === 'ready') refreshThread(read); }, [read]);
  useReadAgain(turnShown && shown, readNow, RUNNING_READ_AGAIN_MS);
  useEffect(() => {
    if (!turnShown || !shown) return;
    document.addEventListener('visibilitychange', readNow);
    return () => document.removeEventListener('visibilitychange', readNow);
  }, [turnShown, shown, readNow]);
  const connected = useSocketStatus().phase === 'ready';
  const wasConnected = useRef(connected);
  useEffect(() => {
    if (connected && !wasConnected.current && shown) readNow();
    wasConnected.current = connected;
  }, [connected, shown, readNow]);

  // Escape brings the main conversation back from a sub-agent's, where nothing else is using the key.
  useEffect(() => {
    if (agent === null || onAgent === undefined) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return;
      if (instance !== null && (!(event.target instanceof Node) || instance.root.current?.contains(event.target) !== true)) return;
      const target = event.target instanceof HTMLElement ? event.target : null;
      if (target !== null && target.closest('input, textarea, select, [role="dialog"], [role="menu"], [role="listbox"]') !== null) return;
      event.preventDefault();
      onAgent(null);
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [agent, onAgent, instance]);

  const sending = useSending(threadId);
  // What waits: a message that the thread takes into a turn of its own is a message in the conversation; the queue holds the rest.
  const waiting = partitionWaiting(view, sending);
  // Where the reader stands in the conversation, told by the list; the round button above the dock reads it.
  const [following, setFollowing] = useState<FollowState>({ atEnd: true, away: 0, ahead: false });
  // After Send the sent message is brought to the top of the view and the view stays there while the reply grows: following waits
  // until the reader has been away from the end and is at it again, or goes there with the round button.
  const [sendCount, setSendCount] = useState(0);
  const [sent, setSent] = useState<{ count: number; hold: boolean } | null>(null);
  const [released, setReleased] = useState(0);
  const [wasAway, setWasAway] = useState(false);
  if ((sent?.count ?? 0) !== sendCount) {
    setSent({ count: sendCount, hold: waiting.message !== null });
    setWasAway(false);
  }
  const holding = sent !== null && sent.hold && released !== sent.count;
  if (holding) {
    if (!following.atEnd && !wasAway) setWasAway(true);
    else if (following.atEnd && wasAway) {
      setReleased(sent.count);
      setWasAway(false);
    }
  }
  const room = useReplyRoom(viewport, spacer, holding);
  // The people's messages and where the reader is among them: the keys, the arrows, the Messages list and the sticky prompt read it.
  const [top, setTop] = useState<string | null>(null);
  const [inView, setInView] = useState<ReadonlySet<string>>(NOTHING_IN_VIEW);
  const nav = useMessageNav(threadId, { top, visible: inView },
    (id) => timeline.current?.scrollToItem(id, { align: 'start', highlight: false }));
  useMessageKeys(nav, shown && focusedContent && agent === null);
  useLayoutEffect(() => {
    if (sent === null || sent.count === 0) return;
    const block = sent.hold ? viewport?.querySelector<HTMLElement>('[data-sent-message]') : null;
    if (viewport !== null && block !== null && block !== undefined) room.hold(viewport, block);
    else timeline.current?.scrollToEnd();
  }, [sent, viewport, room]);
  const unarchive = useMutation({
    mutationFn: () => unarchiveNode(threadId),
  });
  const limits = useQuery({ ...limitsQuery(threadId), enabled: proceeding });

  if (view.closed !== null) {
    return (
      <EmptyState
        className="h-full"
        icon={<MessageSquareText aria-hidden />}
        title={view.closed === 'deleted' ? t('thread.screen.deleted') : t('thread.screen.revoked')}
      />
    );
  }
  if (thread === null) {
    if (view.thread !== null && view.error === null) return <ErrorState className="h-full" error={null} message={t('thread.screen.incomplete')}
      action={<Button size="sm" onClick={() => reloadThread(read)}>{t('common.tryAgain')}</Button>} />;
    if (view.error !== null) {
      // A thread the server refused shows what is missing and whom to ask, and the way on, as every screen of a place does.
      if (view.error instanceof ApiError && view.error.is('authz.denied')) {
        return <NoAccess error={view.error} className="h-full" forThread={threadId} onRetry={() => reloadThread(read)} />;
      }
      return <ErrorState className="h-full" title={t('thread.screen.loadFailed')} error={view.error} onRetry={() => reloadThread(read)} />;
    }
    return <ThreadSkeleton />;
  }

  // The step of the sub-agent shown in place of the conversation names it; one the page does not hold yet is named by what it is.
  const agentStep = agent === null ? undefined : view.items.get(agent) ?? selectedParent.data;
  const agentStream = agentStep?.kind === 'tool_call' && agentStep.payload.tool.kind === 'subagent' ? agentStep.payload.tool.child_stream : undefined;
  const shownAgent = agent === null ? null : { id: agent, name: (agentStep?.kind === 'tool_call' && agentStep.payload.tool.kind === 'subagent' ? agentStep.payload.tool.name : null) ?? t('timeline.step.subagentUnnamed') };
  const mayDrive = holds(thread.access, 'thread.interact');
  const composerShown = !reading.readOnly && !thread.archived;
  const drivingShown = composerShown && mayDrive && thread.input_access.allowed;
  // The composer carries the Stop of a running turn; where it is not drawn (a sub-agent's conversation, a selection being made) the status line does.
  const composerDrawn = agent === null && !(picking.active && !reading.readOnly) && (composerShown || holdBox);

  return (
    <PersonThingProvider thing={{ kind: 'thread', id: threadId }}><ThreadNowProvider threadId={threadId}><AgentSwitchContext.Provider value={onAgent ?? null}><div ref={frame} className="flex h-full min-h-0 flex-col overflow-hidden" data-screen="thread">
      <ThreadHeader thread={thread} recoverySwitchVisible={threadSwitchRecovery(thread, reading.readOnly) !== undefined} onRevealItem={(id) => timeline.current?.scrollToItem(id)} notes={notesHost} nav={nav} agent={shownAgent} onAgent={onAgent} />
      <FindInConversation key={agent ?? 'main'} threadId={threadId} stream={agent === null ? 'main' : agentStream} enabled={agent === null || agentStream !== undefined} timeline={agent === null ? timeline : childTimeline} root={frame} />
      <div className="relative flex min-h-16 flex-1 flex-col">
        <div ref={setViewport} className="flex min-h-0 flex-1 flex-col overflow-x-hidden overflow-y-auto" data-scroll-area>
          <div ref={setNotesHost} className="flex min-w-0 shrink-0 flex-col gap-1 px-4 pt-2 empty:hidden" data-thread-notes />
          <LocalAddresses environmentId={thread.environment?.id ?? null} canView={thread.environment_permissions.includes('ports.view')}>
          {agent === null ? <>
            <AudienceBanner thread={thread} />
            {nav.error !== null && <ErrorAlert error={nav.error} action={<Button size="sm" variant="ghost" onClick={nav.retry}>{t('common.tryAgain')}</Button>} />}
            <Frame><ThreadSummaryInspector threadId={threadId} enabled={shown} /></Frame>
            {!reading.readOnly && thread.environment !== null && thread.binding !== null && <div className="min-w-0 shrink-0 px-4 pt-1">
              <ThreadGatewayStatus threadId={threadId} executionRevision={thread.execution_revision} line />
            </div>}
            {linkedTurn.isError && <ErrorAlert error={linkedTurn.error} />}
            {/* The thread's README stands above its conversation, folded to one line once it was read. */}
            {thread.readme !== null && (
              <div className="px-4 pt-2">
                <PlaceReadme readme={thread.readme} nodeId={thread.id} foldable />
              </div>
            )}
            {/* Virtual rows share the view with what stands above them; the conversation retains its minimum row. */}
            <div className="min-h-16 flex-auto shrink-0">
              {/* A local address an agent writes leads a reader of the environment's Ports door to that port. */}
                {viewport !== null && <Timeline
                  key={windowKey}
                  ref={timeline}
                  viewport={viewport}
                  threadId={threadId}
                  item={item ?? promptItem ?? linkedTurn.data?.items[0]?.id ?? null}
                  items={items}
                  pinApprovals={composerShown}
                  selection={selection}
                  follow={!holding}
                  onFollow={setFollowing}
                  onTop={setTop}
                  onVisible={setInView}
                  onVersionSelect={onVersionSelect}
                  focusPrompt={focusPrompt}
                  highlightLink={prompt === undefined}
                  empty={thread.binding === null && thread.program?.id !== 'orch' ? <p className="text-sm text-text-muted">{t('thread.screen.unbound')}</p> : <BindingSummary thread={thread} />}
                />}
            </div>
            {waiting.message !== null && <SentMessageBlock key={waiting.message.key} threadId={threadId} message={waiting.message} />}
          </> : (
            <div className="mx-auto w-full max-w-(--reading-width) px-4 py-3"><SubagentConversation threadId={threadId} itemId={agent} ref={childTimeline} /></div>
          )}
          </LocalAddresses>
          <div ref={setSpacer} aria-hidden className="shrink-0" />
        </div>
        {agent === null && <StickyPrompt nav={nav} />}
        {agent === null && (!following.atEnd || following.ahead) && <FollowButton away={following.away} onClick={() => {
          room.free();
          if (sent !== null) setReleased(sent.count);
          timeline.current?.scrollToEnd();
        }} />}
      </div>
      {/* The dock stays under the conversation. What stands above the composer scrolls inside itself when the screen has no more room; the line and the composer never do (the box's own menus stand above it, outside it). */}
      <div className="flex min-h-0 shrink flex-col" data-thread-dock>
        <div className="min-h-0 shrink overflow-y-auto">
        {shown && <PendingAgentSchedules thread={thread} readOnly={reading.readOnly} />}
        <Frame className="pt-2 pb-0">
          {/* What says how things stand takes one row, the most urgent first; what waits for an answer keeps its own. */}
          <PassiveStrip entries={[
            ...(reading.readOnly ? [] : [
              { id: 'blocked', node: <BlockedBanner thread={thread} onProceed={() => setProceeding(true)} /> },
              { id: 'lock', node: <LockBanner thread={thread} /> },
            ]),
            { id: 'connection', node: <ConnectionBanner threadId={threadId} /> },
            { id: 'goal', node: <GoalLine thread={thread} /> },
            { id: 'continuation', node: <ContinuationNotice key={windowKey} threadId={threadId} view={view} onSelect={onVersionSelect} /> },
            ...(reading.readOnly ? [] : [{ id: 'source', node: <SourcePreparation thread={thread} /> }]),
            ...(drivingShown && !picking.active ? [{ id: 'review', node: <ReviewTray threadId={threadId} /> }] : []),
          ]} />
          {composerShown && <PinnedApprovals threadId={threadId} />}
          <QueueTray threadId={threadId} queued={waiting.queued} sending={waiting.sending} readOnly={reading.readOnly} />
          <PlanLine threadId={threadId} onJump={(id) => timeline.current?.scrollToItem(id)} />
        </Frame>
        </div>
        <Frame className="pt-2">
          <StatusLine threadId={threadId} withStop={!composerDrawn} />
          {agent !== null ? (
            <div className="flex flex-wrap items-center gap-2 rounded-lg border border-border bg-bg-subtle px-3 py-2 text-sm text-text-muted" data-agent-readonly>
              <Lock className="shrink-0" aria-hidden />
              <span className="min-w-0 flex-1 truncate">{t('thread.agents.readOnly', { name: shownAgent?.name ?? '' })}</span>
              <Button size="sm" variant="ghost" onClick={() => onAgent?.(null)}>{t('thread.agents.backToMain')}</Button>
            </div>
          ) : picking.active && !reading.readOnly ? (
            <TimelineSelectionBar threadId={threadId} ids={picking.ids} canQuote={drivingShown} onCancel={picking.clear} />
          ) : reading.readOnly && !holdBox ? <ThreadRunsLine thread={thread} /> : thread.archived ? (
            <>
              <div className="flex flex-wrap items-center gap-3 rounded-lg border border-border bg-bg-subtle px-4 py-3 text-sm text-text">
                <Archive className="text-text-muted" aria-hidden />
                <span className="flex-1">{thread.archived_by === null ? t('thread.archived.lineAnonymous') : t('thread.archived.line', { name: thread.archived_by.display_name })}</span>
                {threadCapabilities(thread)?.archive === true ? (
                  <Button size="sm" pending={unarchive.isPending} onClick={() => unarchive.mutate()}>
                    {t('thread.archived.unarchive')}
                  </Button>
                ) : (
                  <span className="text-text-muted">{t('thread.archived.who', { who: thread.owner.display_name })}</span>
                )}
                {unarchive.error !== null && <ErrorAlert error={unarchive.error} className="w-full" />}
              </div>
              <ThreadRunsLine thread={thread} />
            </>
          ) : composerShown || holdBox ? (
            <Composer thread={thread} onSent={() => setSendCount((count) => count + 1)} />
          ) : null}
        </Frame>
      </div>
      <GoalEditorHost thread={thread} />
      {proceeding && limits.data !== undefined && (
        <LimitPrompt
          threadId={threadId}
          open
          limits={limits.data}
          pending={false}
          onOpenChange={(open) => !open && setProceeding(false)}
          onSend={(bypass) => {
            // The next message the person sends goes past what they have just read.
            composerWriter.letNextPass(threadId, bypass);
            setProceeding(false);
          }}
        />
      )}
    </div></AgentSwitchContext.Provider></ThreadNowProvider></PersonThingProvider>
  );
}
