/** A page supplies context; each conversation retains one root org for its entire life. */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { History, Plus, Bookmark, Download, ChevronRight } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { askHistoryQuery, askKey, askModelsQuery, askQuery, askReader, createAsk, exportAsk, olderAsk, sendAsk, stopAsk } from '@/api/ask';
import { adminModeKey } from '@/api/adminMode';
import { saveFile } from '@/features/export/save';
import { meQuery } from '@/api/auth';
import { client } from '@/api/client';
import { orgsQuery } from '@/api/nodes';
import { queryClient } from '@/api/query';
import { Composer } from '@/features/composer/Composer';
import { allocatedAskDraft, askCreationIntent, askDraftKey, askSendIntent, completeAskSendIntent, newAskDraft, registerAskDraft } from '@/features/composer/askDrafts';
import { openHelpReading, useHelpOpener } from '@/features/help/open';
import { AdminModeEnableAction, useAdminMode } from '@/features/access/AdminMode';
import { closeAsk, openAskConversation } from './open';
import { applyAskDraftSeed, askSeed } from './seed';
import { useAskRoot } from './useAskRoot';
import { KnownNodeChip, TypedChip } from '@/features/people/NamedChip';
import { contentAddress } from '@/features/workbench/contentAddress';
import { findTab, setTabTarget } from '@/features/workbench/layout';
import { openContent } from '@/features/workbench/contentNavigation';
import { useWorkbench } from '@/features/workbench/store';
import { ViewWindow } from '@/features/workbench/ViewWindow';
import { useViewShown, type ViewProps } from '@/features/workbench/views';
import type { AdminMode, AskGetReply, AskModel, ContentPart } from '@/generated/api-types';
import { useNodePath } from '@/realtime/stores/tree';
import { useCurrentHeading } from '@/shell/screen';
import { useSelectedOrg } from '@/shell/org';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { StateLine } from '@/ui/StateLine';
import { UserChip } from '@/ui/UserChip';
import { HelpHint } from '@/ui/HelpHint';
import { AskMessageRow, readOnlyStep } from './Conversation';
import { AskHistory } from './History';
import { KeepAsk } from './Keep';
import { useConversation } from './useConversation';
import { AskEmptyState, AskGovernance, AskStatus, AskToolProgress } from './States';
import { AskModelAvailability } from './ModelAvailability';

export function AskView({ tab }: ViewProps) {
  const reader = useStore(client.session, (state) => `${state.userId ?? ''}:${state.readerVersion}`);
  return <AskPanel key={reader} tab={tab} />;
}

function AskPanel({ tab }: Pick<ViewProps, 'tab'>) {
  const { t } = useTranslation();
  const openHelp = useHelpOpener();
  const shown = useViewShown();
  const adminMode = useAdminMode();
  const inspecting = tab.of?.inspect === '1';
  const admitted = !inspecting || adminMode.data?.as_admin === true;
  const heading = useCurrentHeading();
  const path = useNodePath(heading?.nodeId);
  const selected = useSelectedOrg((state) => state.orgId);
  const orgs = useQuery({ ...orgsQuery, enabled: shown }).data ?? [];
  const me = useQuery({ ...meQuery, enabled: shown }).data;
  const frame = useWorkbench();
  const page = frame.activeContent === null ? null : findTab(frame.layout, frame.activeContent)?.tab;
  const pageAddress = page === null || page === undefined ? undefined : contentAddress(page) ?? undefined;
  const seed = useStore(askSeed, (state) => state.seed);
  const selectedOrg = orgs.find((org) => org.id === selected);
  const pageRoot = path.orgId ?? heading?.path?.find((part) => part.kind === 'org')?.id ?? selectedOrg?.org_id ?? selected ?? orgs.find((org) => org.org_kind === 'personal')?.id ?? null;
  const [chosenRoot, chooseRoot] = useAskRoot(pageRoot, seed);
  const [selections, setSelections] = useState<Record<string, string | null>>({});
  const history = useInfiniteQuery({ ...askHistoryQuery(chosenRoot ?? ''), enabled: shown && chosenRoot !== null && tab.of?.conversation === undefined });
  const currentId = tab.of?.conversation ?? (chosenRoot === null ? null : Object.hasOwn(selections, chosenRoot) ? selections[chosenRoot] ?? null : history.data?.pages[0]?.items[0]?.id ?? null);
  const conversation = useConversation(currentId, shown && admitted, inspecting);
  const snapshot = admitted ? conversation.data : undefined;
  const header = snapshot?.conversation;
  const root = header?.root_org_id ?? chosenRoot;
  const modelOptions = useQuery({ ...askModelsQuery(root ?? ''), enabled: shown && !inspecting && root !== null });
  const modelsAvailable = modelOptions.isSuccess && modelOptions.data.blocked_reason === null;
  const rootName = orgs.find((org) => org.id === root)?.name;
  const contextReady = heading !== null && heading.tabPending !== true && heading.views?.ready !== false;
  const context = contextReady && root === pageRoot ? pageAddress : undefined;
  const [historyOpen, setHistoryOpen] = useState(false), [keepOpen, setKeepOpen] = useState(false);
  const [models, setModels] = useState<Record<string, AskModel | null>>({});
  const [creatingCount, setCreatingCount] = useState(0);
  const creating = creatingCount > 0;
  const [failure, setFailure] = useState<unknown>(null);
  const [stopped, setStopped] = useState<string | null>(null);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const [retrying, setRetrying] = useState(false);
  const retryPending = useRef(false);
  const [exporting, setExporting] = useState(false);
  const [stopping, setStopping] = useState(false);
  const transcript = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const latest = snapshot?.messages.at(-1);
  const latestContent = latest === undefined ? '' : JSON.stringify(latest.content);
  useEffect(() => { following.current = true; }, [currentId]);
  useEffect(() => { if (following.current && transcript.current !== null) transcript.current.scrollTop = transcript.current.scrollHeight; }, [currentId, latestContent]);
  const starts = useRef(new Map<string, Promise<string>>());
  const scope = `${root ?? ''}:${currentId ?? 'new'}`;
  const draftKey = askDraftKey(root ?? '', currentId);
  useEffect(() => {
    if (seed !== null && shown && admitted && !inspecting && root !== null && contextReady && !history.isLoading
      && (currentId === null || header?.can.message === true)) applyAskDraftSeed(seed.id, draftKey, root);
  }, [seed, shown, admitted, inspecting, root, contextReady, history.isLoading, currentId, header?.can.message, draftKey]);
  const model = models[draftKey] === undefined ? header?.model ?? null : models[draftKey] ?? null;
  const ensure = useCallback(async (): Promise<string> => {
    const current = askReader();
    if (root === null) throw new Error(t('ask.noOrg'));
    if (currentId !== null) return currentId;
    const allocated = allocatedAskDraft(draftKey);
    if (allocated !== undefined) { setSelections((before) => (before[root] === null || before[root] === undefined) && askDraftKey(root, null) === draftKey ? { ...before, [root]: allocated } : before); return allocated; }
    let starting = starts.current.get(draftKey);
    if (starting === undefined) {
      // Creation retakes history before its snapshot and local draft alias are ready.
      // Keep this New conversation selected until preparation explicitly selects its identity.
      setSelections((before) => Object.hasOwn(before, root) ? before : { ...before, [root]: null });
      const creation = askCreationIntent(draftKey, { root_org_id: root, context: context ?? null, model, retention_days: null });
      setCreatingCount((count) => count + 1);
      starting = createAsk(creation.body, creation.key)
        .then(async (receipt) => {
          current();
          const created = await queryClient.query(askQuery(receipt.conversation_id));
          current();
          registerAskDraft(created.conversation.root_org_id, receipt.conversation_id, draftKey);
          setSelections((before) => (before[root] === null || before[root] === undefined) && askDraftKey(root, null) === draftKey ? { ...before, [root]: receipt.conversation_id } : before);
          return receipt.conversation_id;
        }).finally(() => { starts.current.delete(draftKey); setCreatingCount((count) => count - 1); });
      starts.current.set(draftKey, starting);
    }
    return starting;
  }, [root, currentId, context, model, draftKey, t]);
  const send = async (content: ContentPart[], clientId: string) => {
    const current = askReader();
    const id = await ensure();
    current();
    const selection = models[draftKey];
    await sendAsk(id, { client_id: clientId, content, context: context ?? null, model: selection === undefined ? model : selection }, selection === undefined);
    current();
    setStopped(null);
    await queryClient.query(askQuery(id));
    void history.refetch();
  };
  const older = async () => {
    if (currentId === null || loadingOlder) return;
    if (snapshot === undefined) return;
    if (snapshot.previous_cursor === null) return;
    setLoadingOlder(true); setFailure(null);
    const currentReader = askReader();
    try {
      const previous = await olderAsk(currentId, snapshot.previous_cursor);
      currentReader();
      if (inspecting && queryClient.getQueryData<AdminMode>(adminModeKey)?.as_admin !== true) return;
      queryClient.setQueryData<AskGetReply>(askKey(currentId, inspecting), (current) => current === undefined ? previous : { ...current, previous_cursor: previous.previous_cursor,
        messages: [...previous.messages.filter((older) => !current.messages.some((message) => message.id === older.id)), ...current.messages] });
    } catch (error) { setFailure(error); } finally { setLoadingOlder(false); }
  };
  const fresh = () => { const nextRoot = tab.of?.conversation === undefined ? root : pageRoot; if (nextRoot !== null) { newAskDraft(nextRoot); frame.change((layout) => setTabTarget(layout, tab.id, {})); setSelections((before) => ({ ...before, [nextRoot]: null })); setFailure(null); } };
  const active = header?.state === 'running';
  const isStopped = header?.state === 'ready' && (latest?.role === 'assistant' ? !latest.complete : stopped === currentId);
  const givenTitle = header?.title;
  const title = givenTitle === '' ? t(inspecting ? 'ask.inspection' : 'ask.newConversation') : givenTitle ?? t(inspecting ? 'ask.inspection' : 'ask.newConversation');
  const disabled = inspecting || !modelsAvailable || root === null || !contextReady || history.isLoading || currentId !== null && (header === undefined || header.state === 'closed' || header.state === 'kept' || header.inspecting || !header.can.message);
  const retryAnswer = (messageId: string, content: ContentPart[]) => {
    if (retryPending.current || disabled || active) return;
    const key = `${draftKey}:retry:${messageId}`;
    // A retry after an uncertain acknowledgement keeps the original context/destination callback,
    // just like the composer. An explicit model/content edit begins a different personal intent.
    const attempt = askSendIntent(key, JSON.stringify([content, model]), (id) => send(content, id));
    retryPending.current = true; setRetrying(true); setFailure(null);
    void attempt.send().then(() => completeAskSendIntent(key, attempt)).catch(setFailure).finally(() => { retryPending.current = false; setRetrying(false); });
  };
  const error = conversation.error ?? (tab.of?.conversation === undefined ? history.error : null);
  const unreadReason = !admitted ? t('ask.inspectEnable') : conversation.deleted ? t('ask.deleted')
    : error !== null ? t('ask.actions.unavailable')
    : history.isLoading || currentId !== null && conversation.isLoading ? t('ask.actions.checking')
    : root === null ? t('ask.noOrg') : t('ask.actions.noConversation');
  const exportReason = header === undefined ? unreadReason : header.can.export ? undefined : t('ask.actions.exportDenied');
  const keepReason = header === undefined ? unreadReason : inspecting || header.inspecting ? t('ask.inspection')
    : active ? t('ask.actions.answerRunning') : header.can.keep ? undefined
    : header.state === 'closed' ? t('ask.closed') : header.state === 'kept' ? t('ask.kept') : t('ask.actions.keepDenied');
  const stop = async () => {
    if (currentId === null || header?.can.stop !== true || stopping) return;
    setStopping(true);
    try { const current = askReader(); await stopAsk(currentId); current(); setStopped(currentId); await conversation.refetch(); }
    finally { setStopping(false); }
  };
  const composer = (available: boolean) => <Composer ask={{ scope, draftKey, root: root ?? '', conversationId: currentId, model, active, disabled: disabled || !available,
    canStop: !inspecting && header !== undefined && header.can.stop && !header.inspecting && !stopping, ensure, send,
    onModel: (model) => setModels((before) => ({ ...before, [draftKey]: model })), stop }} />;
  return <ViewWindow name="ask" tabId={tab.id} title={title} onClose={closeAsk}
    status={header === undefined ? undefined : <AskStatus conversation={header} stopped={isStopped} />}
    actions={<><HelpHint term={t('ask.title')} what={t('ask.helpWhat')} which={t('ask.helpChoice')} guide={{ article: 'ask', heading: 'open-ask' }} more={{ label: t('ask.guide'), onOpen: () => {
      if (document.querySelector('[data-view-window="ask"][data-floating="true"]') !== null) openHelpReading('ask', 'open-ask');
      else openHelp('ask', 'open-ask');
    } }} />
      <IconButton size="sm" label={t('ask.history')} icon={<History aria-hidden />} disabled={creating} onClick={() => setHistoryOpen(true)} />
      <IconButton size="sm" label={t('ask.new')} icon={<Plus aria-hidden />} pending={creating} disabled={root === null || creating} onClick={fresh} />
      <IconButton size="sm" label={t('ask.export')} icon={<Download aria-hidden />} pending={exporting} blocked={exportReason !== undefined}
        reason={exporting ? t('ask.actions.exporting') : exportReason} onClick={() => {
        if (currentId === null || exporting || exportReason !== undefined) return; const current = askReader(); setExporting(true);
        void exportAsk(currentId).then((blob) => { current(); if (inspecting && queryClient.getQueryData<AdminMode>(adminModeKey)?.as_admin !== true) return; saveFile(blob, `${title}.json`); }).catch(setFailure).finally(() => setExporting(false));
      }} />
      <IconButton size="sm" label={t('ask.keep')} icon={<Bookmark aria-hidden />} blocked={keepReason !== undefined} reason={keepReason}
        onClick={() => { if (keepReason === undefined) setKeepOpen(true); }} /></>}>
    <details className="group shrink-0 border-b border-border px-3 py-1 text-sm text-text-muted" data-ask-details>
      <summary className="focus-inset flex min-w-0 cursor-default list-none items-center gap-2 rounded-md py-1 select-none [&::-webkit-details-marker]:hidden">
        <ChevronRight className="shrink-0 text-text-subtle transition-transform group-open:rotate-90" size={14} aria-hidden /><span className="shrink-0">{t('ask.scope')}</span>{root === null ? <span>{t('ask.noOrg')}</span> : rootName === undefined ? <TypedChip reference={{ kind: 'node', id: root, label: t('ask.organization') }} /> : <KnownNodeChip node={{ kind: 'org', id: root, name: rootName, redacted: false }} />}
        {context !== undefined && <span className="min-w-0 truncate" title={heading?.title}>{t('ask.pageContext', { name: heading?.title ?? '' })}</span>}</summary>
      <div className="flex min-w-0 flex-col gap-1 pb-1 pl-[22px]">
      {header !== undefined && <AskGovernance conversation={header} />}
      {header?.payer_id !== undefined && header.payer_id !== null && <div className="flex items-center gap-2"><span>{t('ask.paidBy')}</span>{me?.id === header.payer_id ? <UserChip user={me} size={24} /> : <span>{t(header.payer_id === header.user_id ? 'ask.conversationOwner' : 'ask.settingOwner')}</span>}</div>}
      {header === undefined && <span>{t('ask.readOnlyTools')}</span>}
      </div>
    </details>
    <div ref={transcript} className="min-h-0 flex-1 overflow-y-auto px-3" data-ask-transcript aria-label={t('ask.conversation')}
      onScroll={(event) => { const node = event.currentTarget; following.current = node.scrollHeight - node.clientHeight - node.scrollTop <= 32; }}>
      {!admitted && <StateLine title={t('ask.inspectEnable')} action={<AdminModeEnableAction />} />}
      {!admitted ? null : history.isLoading || currentId !== null && conversation.isLoading || error !== null || conversation.deleted || snapshot === undefined || snapshot.messages.length === 0 ?
        <AskEmptyState loading={history.isLoading || currentId !== null && conversation.isLoading} error={error} deleted={conversation.deleted} retry={() => { void history.refetch(); void conversation.refetch(); }} fresh={fresh} /> : <>
          {snapshot.previous_cursor !== null && <Button size="sm" pending={loadingOlder} onClick={() => void older()}>{t('ask.older')}</Button>}
          <ol>{snapshot.messages.map((message, index) => {
            // A step that only read sources is not an answer: the answer after it carries the same sources.
            if (readOnlyStep(message) && snapshot.messages[index + 1]?.role === 'assistant') return null;
            const input = snapshot.messages.slice(0, index).findLast((one) => one.role === 'user');
            return <AskMessageRow key={message.id} message={message} threadId={snapshot.conversation.id} {...(me !== undefined && me.id === header?.user_id ? { owner: me } : {})} canRetry={header?.can.message === true && !disabled && !active && !retrying && input !== undefined}
              onRetry={() => { if (input !== undefined) retryAnswer(message.id, input.content); }} />;
          })}</ol>
        </>}
      {admitted && conversation.notice !== '' && <StateLine title={conversation.notice} />}
      {admitted && Object.entries(conversation.tools).map(([id, name]) => <AskToolProgress key={id} name={name} />)}
      {admitted && conversation.streamError !== null && <ErrorAlert error={conversation.streamError} action={<Button size="sm" onClick={conversation.reconnect}>{t('common.tryAgain')}</Button>} />}
      {failure !== null && <ErrorAlert error={failure} />}
    </div>
    {inspecting ? composer(true) : <AskModelAvailability root={root ?? ''} stop={active && header.can.stop ? <Button size="sm" pending={stopping} onClick={() => { void stop().catch(setFailure); }}>{t('composer.stop')}</Button> : undefined}>
      {composer}
    </AskModelAvailability>}
    {historyOpen && <AskHistory root={root} onClose={() => setHistoryOpen(false)} onChoose={(conversation, inspection) => {
      if (inspection) { setHistoryOpen(false); openAskConversation(conversation.id); return; }
      frame.change((layout) => setTabTarget(layout, tab.id, {}));
      chooseRoot(conversation.root_org_id); setSelections((before) => ({ ...before, [conversation.root_org_id]: conversation.id })); setHistoryOpen(false);
    }} />}
    {keepOpen && header !== undefined && <KeepAsk conversation={header} onClose={() => setKeepOpen(false)} onKept={(thread) => { setKeepOpen(false); void conversation.refetch(); openContent(`/thread/${thread}`); }} />}
  </ViewWindow>;
}
