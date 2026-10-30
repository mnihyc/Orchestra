/** Header, message list and composer of one chat.
 *
 * One component for every chat. The header names the chat (a place's chat by the place, a direct chat by
 * the person, a group by its name or its people, guests marked) with who is looking, the bell, the
 * pinned messages and, for a group, its menu. Under it the messages, who is typing, and the composer;
 * or, in its place, the bar of a selection being made, or the sentence that says why the reader may not
 * write here. The chat is followed only while it is on screen.
 */
import { useQuery } from '@tanstack/react-query';
import { Lock, MessageSquareOff, Search } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { Tooltip } from '@/ui/Tooltip';
import { useErrorText } from '@/ui/errorText';
import { AdminReachNotice } from '@/features/access/AdminReachNotice';
import { privateAdminAccess } from '@/ui/AsAdminMark';
import { useStore } from 'zustand';

import { fetchMessages } from '@/api/chats';
import { client } from '@/api/client';
import { setReadPosition } from '@/api/people';
import { ChatPeople } from '@/features/people/ChatPeople';
import { orgsQuery } from '@/api/nodes';
import { discard, pin, read, remove, toggleReaction } from '@/features/chat/acts';
import { Bell } from '@/features/chat/Bell';
import { ChatContext, type ChatContextValue, type MessageActions, usePeople } from '@/features/chat/context';
import { Composer } from '@/features/chat/Composer';
import { type Quote, partsOfDraft, useDraft } from '@/features/chat/draft';
import { GroupMenu } from '@/features/chat/GroupMenu';
import { ChatMenu } from '@/features/export/ChatMenu';
import { messagePowers } from '@/features/chat/Message';
import { MessageList } from '@/features/chat/MessageList';
import { Pinned } from '@/features/chat/Pinned';
import { Typing } from '@/features/chat/Typing';
import { textOfParts } from '@/features/chat/text';
import { ForwardDialog, type ForwardSource } from '@/features/forward/ForwardDialog';
import type { Destination } from '@/features/forward/destinations';
import { inChat, readParams } from '@/features/search/params';
import { useViewFind } from '@/features/search/ViewFind';
import { useOpenPath } from '@/features/chat/open';
import { SelectionBar, useSelection } from '@/features/forward/Selection';
import { placeUrl } from '@/features/navigation/places';
import type { Chat, ChatMessage, SearchHit } from '@/generated/api-types';
import { chatWriter, refreshChatStates, reloadChat, useChatEntry, useChatFeed, useChatState, useChatStates, useChatStore } from '@/realtime/stores/chat';
import { IconButton } from '@/ui/IconButton';
import { Avatar } from '@/ui/Avatar';
import { ConfirmDialog } from '@/ui/ConfirmDialog';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { UserChip } from '@/ui/UserChip';
import { SkeletonRows } from '@/ui/Skeleton';
import { copyText } from '@/ui/clipboard';
import { toast } from '@/ui/Toast';

/** How long a message that was jumped to stays lit. */
const LIT_FOR_MS = 2_000;

export interface ChatViewProps {
  chatId: string;
  /** Whether the chat is on screen: only then it is followed, and only then anything counts as read. */
  shown?: boolean;
  /** A message to open at, from a link. */
  message?: string | null;
  /** Something for the end of the header: the panel's Expand. */
  actions?: ReactNode;
  /** The node scope switch shares this header with the chat actions. */
  scope?: ReactNode;
  /**
   * What the header says of the chat. `name`: the chat's own name, as a heading (a screen of chats). `kind`: only what
   * kind of place's chat it is, not as a heading, where the screen beside it names the place. `none`: nothing, where a
   * segment above it says it already.
   */
  heading?: 'name' | 'kind' | 'none';
  /** Told which quotes to put in the composer; the view hands over a function that adds them. */
  onReady?: (api: { quote: (quotes: readonly Quote[]) => void; focus: () => void }) => void;
}

/** Whether the page is in front of its reader: a tab behind another counts nothing as read. */
function usePageVisible(): boolean {
  const [visible, setVisible] = useState(() => typeof document === 'undefined' || document.visibilityState === 'visible');
  useEffect(() => {
    const on = () => setVisible(document.visibilityState === 'visible');
    document.addEventListener('visibilitychange', on);
    return () => document.removeEventListener('visibilitychange', on);
  }, []);
  return visible;
}

export function ChatView({ chatId, shown = true, message = null, actions, scope, heading = 'name', onReady }: ChatViewProps) {
  const { t } = useTranslation();
  useChatFeed(chatId, shown);
  const entry = useChatEntry(chatId);
  const state = useChatState(chatId, entry.chat?.my ?? null);

  // The server ended the subscription: the reader may no longer read it, or it is gone. What was on screen is not left standing.
  if (entry.closed !== null) {
    return (
      <EmptyState
        compact
        className="h-full"
        icon={<Lock aria-hidden />}
        title={t(entry.closed === 'deleted' ? 'chat.gone.deleted' : 'chat.gone.revoked')}
        description={t('chat.gone.about')}
      />
    );
  }
  if (entry.chat === null) {
    if (entry.error !== null) {
      return <ErrorState compact className="h-full" error={entry.error} title={t('chat.couldNotLoad')} onRetry={() => reloadChat(chatId)} />;
    }
    return <SkeletonRows rows={6} className="p-4" />;
  }
  // The position the reader had read to when they opened the chat is kept for as long as it stays open:
  // the line of the new messages must not vanish as they read.
  return (
    <OpenChat
      key={chatId}
      chat={entry.chat}
      shown={shown}
      message={message}
      actions={actions}
      scope={scope}
      heading={heading}
      readTo={state === null ? null : state.last_read_seq}
      {...(onReady === undefined ? {} : { onReady })}
    />
  );
}

function OpenChat({
  chat,
  shown,
  message,
  actions,
  scope,
  heading,
  readTo: opened,
  onReady,
}: {
  chat: Chat;
  shown: boolean;
  message: string | null;
  actions: ReactNode;
  scope: ReactNode;
  heading: NonNullable<ChatViewProps['heading']>;
  readTo: number | null;
  onReady?: ChatViewProps['onReady'];
}) {
  const { t } = useTranslation();
  const describeError = useErrorText();
  const me = useStore(client.session, (state) => state.userId);
  const entry = useChatEntry(chat.id);
  const state = useChatState(chat.id, chat.my);
  const people = usePeople(entry);
  const orgsRead = useQuery(orgsQuery).data;
  const orgs = useMemo(() => orgsRead ?? [], [orgsRead]);
  const visible = usePageVisible();
  const [readTo] = useState(opened);
  const selection = useSelection(entry.order);
  const draft = useDraft(chat.id, chat.can.send, shown);
  const addQuotes = draft.addQuotes;
  const input = useRef<HTMLTextAreaElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [target, setTarget] = useState<string | null>(null);
  const [lit, setLit] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<ChatMessage | null>(null);
  const [deletePending, setDeletePending] = useState(false);
  const [forwarding, setForwarding] = useState<ForwardSource | null>(null);
  const [askDestination, setAskDestination] = useState<Destination | null>(null);
  const [askedThread, setAskedThread] = useState<{ id:string; name:string; agent:string } | null>(null);
  const [markedUnread, setMarkedUnread] = useState(false);
  const suppressReads = useRef(false);
  const observedRevision = useRef(state?.read_revision ?? 0);
  const openPath = useOpenPath();
  useEffect(() => useChatStates.subscribe((current, previous) => {
    const before = previous.states.get(chat.id);
    const next = current.states.get(chat.id);
    const revision = next?.read_revision ?? 0;
    if (revision > Math.max(observedRevision.current, before?.read_revision ?? 0)) {
      suppressReads.current = true; setMarkedUnread(true);
    }
    observedRevision.current = Math.max(observedRevision.current, revision);
  }), [chat.id]);
  useEffect(() => { if (!shown) { suppressReads.current = false; queueMicrotask(() => setMarkedUnread(false)); } }, [shown]);

  const label = useMemo(() => {
    if (chat.kind === 'node') return chat.node?.name ?? '';
    return chatTitle(chat, me, t('chat.title.groupUnnamed'));
  }, [chat, me, t]);

  // ---- going to a message
  const jump = useCallback(
    async (id: string) => {
      if (!(useChatStore.getState().byChat.get(chat.id)?.messages.has(id) ?? false)) {
        try {
          chatWriter.around(chat.id, await fetchMessages(chat.id, { around: id }));
        } catch {
          toast({ tone: 'danger', title: t('chat.failed.jump') });
          return;
        }
      }
      setTarget(id);
    },
    [chat.id, t],
  );
  useEffect(() => {
    // After the first drawing: going to a message may change what is drawn.
    if (message !== null) void Promise.resolve().then(() => jump(message));
  }, [message, jump]);
  const reached = useCallback((id: string) => {
    setTarget(null);
    setLit(id);
  }, []);
  const findHit = useCallback((hit: SearchHit) => {
    if (hit.kind === 'chat_message' && hit.chat.id === chat.id) void jump(hit.message.id);
  }, [chat.id, jump]);
  const find = useViewFind({ root, filters: { chat_id: chat.id, kinds: ['chat_message'] }, label: t('search.find.chat'), onHit: findHit, content: '[role="log"]', enabled: shown });
  useEffect(() => {
    if (lit === null) return;
    const timer = setTimeout(() => setLit(null), LIT_FOR_MS);
    return () => clearTimeout(timer);
  }, [lit]);

  // ---- links and copies
  const linkTo = useCallback(
    (id: string): string => {
      if (chat.kind === 'node' && chat.node !== null) {
        return `${placeUrl({ id: chat.node.id, kind: chat.node.kind })}?panel=chat&chat=${chat.id}&message=${id}`;
      }
      const org = orgs.find((one) => one.id === chat.org_id);
      return `${location.origin}/org/${org?.address ?? chat.org_id}/messages?chat=${chat.id}&message=${id}`;
    },
    [chat, orgs],
  );
  const copy = useCallback((text: string) => void copyText(text), []);

  const acts = useMemo<MessageActions>(
    () => ({
      react: (target_, emoji) => {
        if (me !== null) void toggleReaction(chat.id, target_, emoji, me);
      },
      reply: (target_) => {
        draft.setReplyTo(target_.id);
        input.current?.focus();
      },
      edit: (target_) => {
        setEditing(target_.id);
        void jump(target_.id);
      },
      remove: (target_) => setDeleting(target_),
      pin: (target_, on) => {
        void pin(chat.id, target_.id, on);
      },
      forward: (target_) => setForwarding(forwardSource(chat, [target_])),
      select: (target_) => selection.start(target_.id),
      copyText: (target_) => copy(textOfParts(target_.parts)),
      copyLink: (target_) => copy(linkTo(target_.id)),
      jump: (id) => void jump(id),
      unread: (target_) => {
        suppressReads.current = true; setMarkedUnread(true);
        void setReadPosition(chat.id, target_.seq_created, state?.read_revision ?? 0, true)
          .then(() => refreshChatStates())
          .catch((error: unknown) => { suppressReads.current = false; setMarkedUnread(false); toast({ tone: 'danger', title: t('people.markUnread'), description: describeError(error).message }); });
      },
    }),
    // `draft` and `selection` change with every keystroke and pick; what the actions use of them is stable.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [chat, me, jump, copy, linkTo, draft.setReplyTo, selection.start, state, t],
  );

  const picked = useMemo(() => new Set(selection.ids), [selection.ids]);
  const context = useMemo<ChatContextValue>(
    () => ({
      chat,
      me,
      people,
      acts,
      editing,
      setEditing,
      selection: { active: selection.active, has: (id) => picked.has(id), toggle: selection.toggle },
      lit,
    }),
    [chat, me, people, acts, editing, selection.active, selection.toggle, picked, lit],
  );

  // What the composer can be handed from outside: quotes from a thread, and the focus.
  useEffect(() => {
    onReady?.({
      quote: (quotes) => addQuotes(quotes),
      focus: () => { if (input.current !== null) input.current.focus(); else root.current?.focus(); },
    });
  }, [onReady, addQuotes]);

  // Leaving with a message that was not sent asks first.
  useEffect(() => {
    if (entry.outbox.length === 0) return;
    const ask = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', ask);
    return () => window.removeEventListener('beforeunload', ask);
  }, [entry.outbox.length]);

  const onRead = useCallback((seq: number) => { if (!suppressReads.current && chat.can.reason !== 'inspecting') read(chat.id, seq); }, [chat.id, chat.can.reason]);
  const mine = useMemo(
    () => [...entry.order].reverse().map((id) => entry.messages.get(id)).find((one) => one !== undefined && messagePowers(chat, one, me).edit),
    [entry.order, entry.messages, chat, me],
  );

  const other = chat.kind === 'direct' ? (chat.members ?? []).find((member) => member.id !== me) : undefined;
  const placeholder =
    chat.kind === 'node' ? chat.node?.name ? t('chat.composer.place', { name: chat.node.name }) : t('chat.composer.node') : t('chat.composer.person', { name: label });
  const replyingTo = draft.replyTo === null ? null : (entry.messages.get(draft.replyTo) ?? null);
  const inspecting = chat.can.reason === 'inspecting';
  const watching = entry.presence.filter((entry) => entry.user.id !== me);
  const firstWatcher = watching[0]?.user;

  const confirmDelete = async () => {
    if (deleting === null) return;
    setDeletePending(true);
    const done = await remove(chat.id, deleting.id);
    setDeletePending(false);
    if (done) setDeleting(null);
  };

  const title = chatPageTitle(chat, me, t);
  return (
    <ChatContext.Provider value={context}>
      <div ref={root} tabIndex={-1} data-chat-view={chat.id} className="flex h-full min-h-0 min-w-0 flex-col bg-bg">
        <header data-chat-header className="flex h-(--control-md) min-w-0 shrink-0 items-center gap-1 border-b border-border px-1">
          {scope}
          {heading === 'name' && (
            <div className="min-w-0 flex-1">
              <Tooltip content={title}><h2 tabIndex={0} className="focus-inset truncate rounded-md text-base font-semibold text-text">
                {title}
              </h2></Tooltip>
            </div>
          )}
          {heading === 'kind' && (
            <p title={chat.node?.name ?? ''} className="min-w-0 flex-1 truncate text-base font-semibold text-text">
              {t('chat.title.place', { kind: t(`chat.segment.${chat.node?.kind ?? 'topic'}`) })}
            </p>
          )}
          {heading === 'none' && <span className="flex-1" />}
          <div role="group" aria-label={t('chat.controls')} className="ml-auto flex shrink-0 items-center">
          {firstWatcher !== undefined && <Popover label={t('chat.whoIsHere')} align="end"
            trigger={<IconButton size="sm" icon={<Avatar size={24} text={firstWatcher.avatar.text} color={firstWatcher.avatar.color} label={firstWatcher.display_name} />} label={t('ui.presence.here', { names: watching.map((entry) => entry.user.display_name).join(', ') })} />}>
            <ul className="flex flex-col gap-2">{watching.map((entry) => <li key={entry.user.id}><UserChip user={entry.user} /></li>)}</ul>
          </Popover>}
          {!inspecting && <Bell chat={chat} state={state} />}
          <Pinned chatId={chat.id} onJump={(id) => void jump(id)} />
          <IconButton size="sm" label={t('search.find.chat')} icon={<Search aria-hidden />} onClick={find.show} />
          <IconButton size="sm" action="searchHistory" label={t('screenReview.searchChatHistory')} onClick={() => {
            const params = inChat(readParams(Object.fromEntries(new URLSearchParams(location.search))), chat.id, chat.node?.id ?? chat.org_id);
            openPath(`/search?${new URLSearchParams(Object.entries(params).filter((entry): entry is [string, string] => typeof entry[1] === 'string')).toString()}`);
          }} />
          {chat.kind === 'group' ? <GroupMenu chat={chat} me={me} onLeft={() => reloadChat(chat.id)} /> : <ChatMenu chat={chat} me={me} />}
          {actions}
          </div>
        </header>

        <AdminReachNotice of={chat.can.admin_access?.place} private={privateAdminAccess(chat.can.admin_access)} object={{ kind: 'chat', id: chat.id }} />

        {find.bar}

        <ChatPeople chat={chat} me={me} />
        {markedUnread && <p role="status" className="border-b border-border bg-accent-subtle px-3 py-1 text-xs text-text-muted">{t('people.markedUnread')}</p>}
        {askedThread !== null && <p role="status" className="flex flex-wrap items-center gap-x-2 gap-y-1 border-b border-border px-3 py-1 text-xs text-text-muted"><span>{t('people.asked', { agent: askedThread.agent, thread: askedThread.name })}</span><a href={`/thread/${askedThread.id}`} className="text-accent-text underline">{t('people.openThread')}</a></p>}
        <MessageList
          entry={entry}
          onRestore={partsOfDraft(draft).length > 0 ? undefined : (sent) => { draft.restore?.(sent.parts,sent.replyTo,sent.mentionNames ?? []); discard(chat.id,sent.clientId); requestAnimationFrame(() => input.current?.focus()); }}
          readTo={readTo}
          watching={shown && visible}
          target={target}
          onReached={reached}
          onRead={onRead}
          empty={
            <EmptyState
              compact
              className="h-full"
              icon={<MessageSquareOff aria-hidden />}
              title={
                chat.kind !== 'node' && chat.kind !== 'direct' && !chat.can.send ? t('chat.empty.quiet') : t('chat.empty.heading')
              }
              description={chat.kind === 'node' ? t('chat.empty.nodeAbout', { kind: t(`chat.kind.${chat.node?.kind ?? 'topic'}`) })
                : chat.kind === 'direct' ? t('chat.empty.directAbout', { name: label }) : chat.can.send ? t('chat.empty.groupAbout') : undefined}
            />
          }
        />
        <Typing typing={entry.typing} me={me} chatId={chat.id} />

        {selection.active ? (
          <SelectionBar
            count={selection.ids.length}
            most={20}
            onCancel={selection.clear}
            onCopy={() => {
              const lines = selection.ids
                .map((id) => entry.messages.get(id))
                .filter((one): one is ChatMessage => one !== undefined && !one.deleted)
                .map((one) => `${one.author !== null && 'display_name' in one.author ? one.author.display_name : (one.author?.agent ?? '')}: ${textOfParts(one.parts)}`);
              copy(lines.join('\n\n'));
            }}
            onForward={() =>
              setForwarding(
                forwardSource(
                  chat,
                  selection.ids.map((id) => entry.messages.get(id)).filter((one): one is ChatMessage => one !== undefined && !one.deleted),
                ),
              )
            }
          />
        ) : chat.can.send ? (
          <Composer chat={chat} draft={draft} replyingTo={replyingTo} inputRef={input} placeholder={placeholder} onEditLast={() => mine && acts.edit(mine)} onAsk={(thread, agent, source) => {
            setAskDestination({ key:`thread:${thread.id}`, kind:'thread', threadId:thread.id, label:thread.name ?? t('people.privateThread'), detail:agent });
            setForwarding(forwardSource(chat, [source]));
          }} />
        ) : (
          <Notice chat={chat} personName={other?.display_name ?? label} />
        )}
      </div>

      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(open) => !open && setDeleting(null)}
        title={t('chat.delete.title')}
        confirmLabel={t('chat.delete.confirm')}
        danger
        pending={deletePending}
        onConfirm={() => void confirmDelete()}
      >
        {t('chat.delete.body')}
      </ConfirmDialog>
      <ForwardDialog
        source={forwarding}
        initialDestinations={askDestination === null ? [] : [askDestination]}
        onClose={() => { setForwarding(null); setAskDestination(null); }}
        onForwarded={(reply) => {
          selection.clear();
          if (askDestination?.kind === 'thread' && reply.threads.some((result) => result.thread_id === askDestination.threadId)) setAskedThread({ id:askDestination.threadId, name:askDestination.label, agent:askDestination.detail ?? '' });
        }}
      />
    </ChatContext.Provider>
  );
}

/** The title of a chat among people: the other person, or the group's name, or its people's names. */
export function chatTitle(chat: Pick<Chat, 'kind' | 'name' | 'members'>, me: string | null, unnamed: string): string {
  if (chat.kind === 'group' && chat.name !== null) return chat.name;
  const others = (chat.members ?? []).filter((member) => member.id !== me).map((member) => member.display_name);
  return others.length === 0 ? unnamed : others.join(', ');
}

/** Page and tab captions name the conversation's kind while retaining its own name. */
export function chatPageTitle(chat: Pick<Chat, 'kind' | 'node' | 'name' | 'members'>, me: string | null, t: TFunction): string {
  if (chat.kind === 'node') return t('chat.title.node', { name: chat.node?.name ?? '', kind: t(`chat.kind.${chat.node?.kind ?? 'topic'}`) });
  const name = chatTitle(chat, me, '');
  return name === '' ? t('chat.title.groupUnnamed') : t('chat.title.named', { name });
}

/** What is forwarded when a chat's messages are picked. */
function forwardSource(chat: Chat, messages: readonly ChatMessage[]): ForwardSource {
  return {
    kind: 'chat',
    chatId: chat.id,
    orgId: chat.org_id,
    label: chat.node?.name ?? chat.name ?? '',
    messages,
  };
}

/** In place of the composer: why the reader may not write here. */
function Notice({ chat, personName }: { chat: Chat; personName: string }) {
  const { t } = useTranslation();
  const reason = chat.can.reason;
  const text =
    reason === 'archived'
        ? t('chat.notice.archived')
        : reason === 'disabled'
          ? t('chat.notice.disabled', { kind: t(`chat.kind.${chat.node?.kind ?? 'topic'}`) })
          : reason === 'closed'
            ? t('chat.notice.closed', { name: personName })
            : t('chat.notice.readOnly');
  return (
    <div role="note" className="flex items-center gap-2 border-t border-border bg-bg-subtle px-4 py-3 text-sm text-text-muted">
      <Lock size={16} aria-hidden className="shrink-0" />
      <span>{text}</span>
    </div>
  );
}

