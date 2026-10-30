import { threadIconFacts } from '@/ui/ThreadState';
/** The thread as a screen of the workbench: the conversation with its composer, and its other views in the arrangement chosen.
 *
 * The screen of a thread: its head, the conversation with the agent's work, what stands above the
 * composer (why the thread cannot go on, what holds it, a plan, approvals that wait, the queue), and the
 * composer, or in its place the card for somebody who may only watch, or the line that the thread is
 * archived. The thread is followed live, so the screen is always the thread as it is now, and says when
 * it could not be brought up to date.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { Archive, MessageSquareText, ShieldQuestionMark } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';

import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useViewInstance, useContentFocused } from '@/features/workbench/instance';
import { ApiError, client } from '@/api/client';
import { unarchiveNode } from '@/api/nodes';
import { limitsQuery } from '@/api/threads';
import { selectThreadContinuation } from '@/api/threadRead';
import { viewedItemsQuery } from '@/api/threadHistory';
import { ContinuationNotice } from '@/features/thread/ContinuationNotice';
import { useReadWhenOpened } from '@/features/inbox/Counters';
import { NoAccess } from '@/features/access/NoAccess';
import { readableNavigationRoot } from '@/features/navigation/currentOrg';
import { Composer } from '@/features/composer/Composer';
import { composerWriter, type SendingInput, useSending } from '@/features/composer/Draft';
import { QueueTray } from '@/features/composer/QueueTray';
import { PlaceReadme } from '@/features/documents/Readme';
import { PersonThingProvider } from '@/features/people/personContext';
import { useSelection } from '@/features/forward/Selection';
import { LocalAddresses } from '@/features/ports/LocalAddresses';
import { BindingSummary } from '@/features/thread/BindingSummary';
import { BlockedBanner } from '@/features/thread/BlockedBanner';
import { ConnectionBanner } from '@/features/thread/ConnectionBanner';
import { SourcePreparation } from '@/features/source/Preparation';
import { ReviewTray } from '@/features/source/Review';
import { ThreadHeader } from '@/features/thread/Header';
import { isThreadForScreen, threadCapabilities } from '@/features/thread/cached';
import { GoalEditorHost } from '@/features/goal/GoalEditor';
import { GoalLine } from '@/features/goal/GoalLine';
import { LockBanner } from '@/features/thread/LockBanner';
import { PassiveStrip } from '@/features/thread/PassiveStrip';
import { WatchingCard } from '@/features/thread/WatchingCard';
import type { ItemSelection, PromptFocus, SelectPromptVersion } from '@/features/timeline/context';
import { FindInConversation } from '@/features/timeline/FindInConversation';
import { PlanLine } from '@/features/timeline/PlanLine';
import { queuedInputs, sendingInTray } from '@/features/timeline/rows';
import { TimelineSelectionBar } from '@/features/timeline/SelectionBar';
import { Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import { useTimelineWindowKey } from '@/features/timeline/windowKey';
import { holds } from '@/lib/permissions';
import { reloadThread, useThreadFeed } from '@/realtime/live';
import { threadReadState, useThreadView } from '@/realtime/stores/thread';
import { selectOrg } from '@/shell/org';
import { useScreenHeading } from '@/shell/screen';
import { Button } from '@/ui/Button';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorAlert, ErrorState } from '@/ui/ErrorState';
import { Skeleton, SkeletonGroup } from '@/ui/Skeleton';
import { LimitPrompt } from '@/ui/LimitPrompt';

function Frame({ children }: { children: React.ReactNode }) {
  return <div className="mx-auto flex w-full max-w-(--reading-width) shrink-0 flex-col gap-2 px-4 pb-3">{children}</div>;
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
}

export function ThreadScreen({ threadId, item, items = null, turn, continuation = null, onVersionSelect, prompt, focusPrompt }: ThreadProps) {
  const { t } = useTranslation();
  const userId = useStore(client.session, (state) => state.userId);
  const read = useThreadReadRef(threadId);
  const instance = useViewInstance();
  const focusedContent = useContentFocused();
  const root = useRef<HTMLDivElement>(null);
  const timeline = useRef<TimelineHandle>(null);
  const [visible, setVisible] = useState<ReadonlySet<string>>(new Set());
  const [proceeding, setProceeding] = useState(false);
  useLayoutEffect(() => selectThreadContinuation(read, continuation), [read, continuation]);
  useThreadFeed(threadId);
  const view = useThreadView(threadId);
  const windowKey = useTimelineWindowKey(view);
  const reading = threadReadState(view);
  const promptItem = prompt === undefined ? null : (view.viewedPrompt?.input_id === prompt ? view.viewedPrompt.item_id : null)
    ?? [...view.items.values()].find((one) => one.kind === 'user_message' && one.payload.input_id === prompt)?.id ?? null;
  const linkedOptions = viewedItemsQuery(read, { turn: turn ?? 0 });
  const linkedTurn = useQuery({ ...linkedOptions, enabled: turn !== undefined && item === null && linkedOptions.enabled, retry: false });
  // Only the actually displayed conversation marks its notifications read; frame metadata does not.
  useReadWhenOpened(!reading.readOnly && view.closed === null && view.loaded !== 'none' ? threadId : null);
  const thread = isThreadForScreen(view.thread, threadId) ? view.thread : null;
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

  const sending = useSending(threadId);
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

  const mayDrive = holds(thread.access, 'thread.interact');
  const pendingApprovals = view.order.flatMap((id) => {
    const found = view.items.get(id);
    return found?.kind === 'approval' && found.status === 'pending' ? [found] : [];
  });
  const offscreen = pendingApprovals.filter((found) => !visible.has(found.id));
  const composerShown = !reading.readOnly && !thread.archived && mayDrive && thread.input_access.allowed;

  return (
    <PersonThingProvider thing={{ kind: 'thread', id: threadId }}><div ref={root} className="flex h-full min-h-0 flex-col overflow-x-hidden overflow-y-auto" data-screen="thread">
      <ThreadHeader thread={thread} onRevealItem={(id) => timeline.current?.scrollToItem(id)} />
      {linkedTurn.isError && <ErrorAlert error={linkedTurn.error} />}
      <FindInConversation threadId={threadId} timeline={timeline} root={root} />
      {/* The thread's README stands above its conversation, folded to one line once it was read. */}
      {thread.readme !== null && (
        <div className="px-4 pt-2">
          <PlaceReadme readme={thread.readme} foldable />
        </div>
      )}
      {/* A small cell retains one ordinary message row; excess controls scroll in this view. */}
      <div className="min-h-16 flex-1">
        {/* A local address an agent writes leads a reader of the environment's Ports door to that port. */}
        <LocalAddresses environmentId={thread.environment?.id ?? null} canView={thread.environment_permissions.includes('ports.view')}>
          <Timeline
            key={windowKey}
            ref={timeline}
            threadId={threadId}
            item={item ?? promptItem ?? linkedTurn.data?.items[0]?.id ?? null}
            items={items}
            onVisible={setVisible}
            selection={selection}
            onVersionSelect={onVersionSelect}
            focusPrompt={focusPrompt}
            highlightLink={prompt === undefined}
            empty={thread.binding === null ? <p className="text-sm text-text-muted">{t('thread.screen.unbound')}</p> : <BindingSummary thread={thread} />}
          />
        </LocalAddresses>
      </div>
      <Frame>
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
          ...(composerShown && !picking.active ? [{ id: 'review', node: <ReviewTray threadId={threadId} /> }] : []),
        ]} />
        {offscreen.length > 0 && (
          <div role="status" className="flex items-center gap-2 rounded-lg border border-warning/40 bg-warning-subtle px-3 py-1 text-sm text-text">
            <ShieldQuestionMark className="text-warning" aria-hidden />
            <span className="flex-1">{t('approvals.barWaiting', { count: pendingApprovals.length })}</span>
            <Button size="sm" onClick={() => timeline.current?.scrollToItem(offscreen[0]?.id ?? '')}>
              {t('approvals.review')}
            </Button>
          </div>
        )}
        <PlanLine threadId={threadId} onJump={(id) => timeline.current?.scrollToItem(id)} />
        <QueueTray threadId={threadId} queued={queuedInputs(view)} sending={sendingTray(view, sending)} readOnly={reading.readOnly} />
        {picking.active && !reading.readOnly ? (
          <TimelineSelectionBar threadId={threadId} ids={picking.ids} canQuote={composerShown} onCancel={picking.clear} />
        ) : reading.readOnly ? null : thread.archived ? (
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
        ) : composerShown ? (
          <Composer thread={thread} onSent={() => timeline.current?.scrollToEnd()} />
        ) : (
          <WatchingCard thread={thread} />
        )}
      </Frame>
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
    </div></PersonThingProvider>
  );
}

function sendingTray(view: ReturnType<typeof useThreadView>, sending: readonly SendingInput[]): readonly SendingInput[] {
  return sendingInTray(view, sending);
}
