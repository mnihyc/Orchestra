import { threadIconFacts } from '@/ui/ThreadState';
/** Result tabs read a document or a thread through its existing live reader without replacing the route's conversation. */
import { contentThreadId } from '@/features/workbench/contentAddress';
import { useQuery } from '@tanstack/react-query';
import { FileText, Lock, MessageSquareText, Search } from 'lucide-react';
import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { nodeQuery, orgBySlugQuery } from '@/api/nodes';
import { ITEM_KINDS } from '@/api/search';
import { selectThreadContinuation } from '@/api/threadRead';
import type { EditorApi } from '@/features/documents/Editor';
import { useViewFind } from '@/features/search/ViewFind';
import { Timeline, type TimelineHandle } from '@/features/timeline/Timeline';
import { useTimelineWindowKey } from '@/features/timeline/windowKey';
import { timelineUi } from '@/features/timeline/state';
import type { PromptFocus, SelectPromptVersion } from '@/features/timeline/context';
import { ContinuationNotice } from '@/features/thread/ContinuationNotice';
import { QueueTray } from '@/features/composer/QueueTray';
import { queuedInputs } from '@/features/timeline/rows';
import { allTabs } from '@/features/workbench/layout';
import { useTabName, useWorkbench } from '@/features/workbench/store';
import type { SearchHit } from '@/generated/api-types';
import { forgetThreadRead, reloadDocument, reloadThread, useDocumentFeed, useThreadFeed } from '@/realtime/live';
import { useDocumentState } from '@/realtime/stores/document';
import { threadViewOf, useThreadView } from '@/realtime/stores/thread';
import { resultThreadRead, sameThreadProjection, type ThreadReadRef } from '@/realtime/threadScope';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { SkeletonRows } from '@/ui/Skeleton';

// A thread preview has no need for a document editor or chat composer.
const BlockEditor = lazy(() => import('@/features/documents/Editor').then((module) => ({ default: module.BlockEditor })));
const ChatView = lazy(() => import('@/features/chat/ChatView').then((module) => ({ default: module.ChatView })));

/** A place's second finder act opens its chat through the ordinary visible place reader. */
export function ResultPlaceChat({ nodeId, tabId, shown }: { nodeId: string; tabId: string; shown: boolean }) {
  const identifier = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(nodeId);
  const node = useQuery({ ...nodeQuery(nodeId), enabled: shown && identifier });
  const org = useQuery({ ...orgBySlugQuery(nodeId), enabled: shown && !identifier });
  const place = node.data ?? org.data;
  const error = node.error ?? org.error;
  useTabName(tabId, error === null ? place?.name : null, MessageSquareText);
  if (error !== null) return <ErrorState error={error} onRetry={() => { if (identifier) void node.refetch(); else void org.refetch(); }} />;
  if (place === undefined) return <SkeletonRows rows={4} />;
  return <Suspense fallback={<SkeletonRows rows={4} />}><ChatView chatId={place.chat_id} shown={shown} /></Suspense>;
}

/** A current document's blocks, read beside a conversation; opening its route provides editing and comments. */
export function ResultDocument({ documentId, block, tabId, shown }: { documentId: string; block: string | null; tabId: string; shown: boolean }) {
  const { t } = useTranslation();
  useDocumentFeed(shown ? documentId : null);
  const state = useDocumentState(documentId);
  const doc = state.document;
  const root = useRef<HTMLDivElement>(null);
  const [editor, setEditor] = useState<EditorApi | null>(null);
  useTabName(tabId, state.ended === null ? doc?.title : null, FileText);
  const bring = useCallback((id: string | null) => {
    const element = root.current;
    if (element === null) return;
    const selector = id === null ? 'h2' : `[data-id="${CSS.escape(id)}"]`;
    element.querySelector<HTMLElement>(selector)?.scrollIntoView({ block: 'center' });
  }, []);
  const findHit = useCallback((hit: SearchHit) => {
    if (editor !== null && hit.kind === 'document' && hit.document.id === documentId) bring(hit.block_id);
  }, [bring, documentId, editor]);
  const find = useViewFind({ root, filters: { document_id: documentId, kinds: ['document'] }, label: t('search.find.document'), onHit: findHit, content: 'article', enabled: shown && doc !== null && state.ended === null });
  const availableDocument = state.ended === null ? doc?.id : undefined;
  useEffect(() => {
    if (availableDocument !== undefined && editor !== null) bring(block);
  }, [block, bring, availableDocument, editor]);
  if (state.ended !== null) return <EmptyState compact icon={<Lock aria-hidden />} title={t('search.beside.unavailable')} />;
  if (doc === null) return state.error === null ? <SkeletonRows rows={6} /> : <ErrorState error={state.error} onRetry={() => reloadDocument(documentId)} />;
  return <div ref={root} tabIndex={-1} className="flex h-full min-h-0 flex-col" data-result-document={documentId}>
    <header className="flex shrink-0 items-center justify-end border-b border-border p-2"><IconButton label={t('search.find.document')} icon={<Search aria-hidden />} onClick={find.show} /></header>
    {find.bar}
    <article className="min-h-0 flex-1 overflow-auto p-4"><h2 className="mb-4 text-xl font-semibold">{doc.title}</h2><Suspense fallback={<SkeletonRows rows={6} />}><BlockEditor document={doc} editable={false} inspection onReady={setEditor} /></Suspense></article>
  </div>;
}

/** Another thread's actual live stream has its own scope, Find and scroll position. */
interface ResultThreadProps {
  threadId: string; item: string | null; tabId: string; shown: boolean;
  continuationId?: string | null;
  onVersionSelect?: SelectPromptVersion | undefined;
  prompt?: string | undefined;
  focusPrompt?: PromptFocus | null | undefined;
}

export function ResultThread(props: ResultThreadProps) {
  const read = useMemo(() => resultThreadRead(props.threadId, props.tabId), [props.threadId, props.tabId]);
  return <ThreadReadProvider read={read}><ThreadResultBody {...props} read={read} /></ThreadReadProvider>;
}

function ThreadResultBody({ threadId, item, tabId, shown, continuationId = null, onVersionSelect, read, prompt, focusPrompt }: ResultThreadProps & { read: ThreadReadRef }) {
  const { t } = useTranslation();
  useLayoutEffect(() => { if (shown) selectThreadContinuation(read, continuationId); }, [read, continuationId, shown]);
  useThreadFeed(shown ? read : null);
  const view = useThreadView(read);
  const windowKey = useTimelineWindowKey(view);
  const promptItem = prompt === undefined ? null : (view.viewedPrompt?.input_id === prompt ? view.viewedPrompt.item_id : null)
    ?? [...view.items.values()].find((one) => one.kind === 'user_message' && one.payload.input_id === prompt)?.id ?? null;
  useEffect(() => () => {
    queueMicrotask(() => {
      if (!allTabs(useWorkbench.getState().layout).some((tab) => tab.id === tabId && contentThreadId(tab) === threadId)) {
        forgetThreadRead(read); timelineUi.forget(read);
      }
    });
  }, [read, tabId, threadId]);
  const root = useRef<HTMLDivElement>(null);
  const timeline = useRef<TimelineHandle>(null);
  useTabName(tabId, view.closed === null ? view.thread?.name : null, MessageSquareText, undefined, false, threadIconFacts(view.closed === null ? view.thread : undefined));
  const findHit = useCallback((hit: SearchHit) => {
    if (hit.kind !== 'item' || hit.thread.id !== threadId) return;
    const projection = threadViewOf(read).read?.projection;
    if (projection !== undefined && projection !== null && (hit.projection === undefined || hit.projection === null || !sameThreadProjection(projection, hit.projection))) return;
    timeline.current?.scrollToItem(hit.item.id);
  }, [threadId, read]);
  const find = useViewFind({ root, filters: { scope: threadId, kinds: ITEM_KINDS }, historyThread: read, label: t('search.find.label'), onHit: findHit, content: '[role="log"]', enabled: shown && view.thread !== null && view.closed === null });
  if (view.closed !== null) return <EmptyState compact icon={<Lock aria-hidden />} title={t('search.beside.unavailable')} />;
  if (view.error !== null && view.thread === null) return <ErrorState error={view.error} onRetry={() => reloadThread(read)} />;
  return <div ref={root} tabIndex={-1} className="flex h-full min-h-0 flex-col" data-result-thread={threadId}>
    <header className="flex shrink-0 items-center gap-2 border-b border-border p-2"><h2 className="min-w-0 flex-1 truncate font-semibold">{view.thread?.name}</h2><IconButton label={t('search.find.label')} icon={<Search aria-hidden />} onClick={find.show} /></header>
    {find.bar}<div className="min-h-0 flex-1"><Timeline key={windowKey} ref={timeline} threadId={threadId} shown={shown} item={item ?? promptItem} empty={null} onVersionSelect={onVersionSelect} focusPrompt={focusPrompt} highlightLink={prompt === undefined} /></div>
    {view.read?.selection !== undefined && view.read.selection !== null && <div className="shrink-0 px-2 pb-2"><ContinuationNotice key={windowKey} threadId={threadId} view={view} onSelect={onVersionSelect} /></div>}
    {view.inputs.size > 0 && <div className="shrink-0 px-2 pb-2"><QueueTray threadId={threadId} queued={queuedInputs(view)} sending={[]} readOnly /></div>}
  </div>;
}
