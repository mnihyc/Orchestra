/** The Chat view, with the nearest thread, topic and project chats.
 *
 * The view of the workbench that holds the chat of the node a screen is of: on a thread a segmented
 * control moves between "Thread · Topic · Project", each segment with its own unread dot; on a topic between
 * "Topic · Project"; on a project and on an org's home it is the one chat of that place. Each chat has its
 * own composer, and what is typed in one is sent to that one alone. "Expand" opens the chat as an
 * overlay four fifths of the width. The address can name the panel, the chat and a message: the panel
 * then comes to the front and opens at the message.
 */
import { useViewInstance } from '@/features/workbench/instance';
import { useWorkbench } from '@/features/workbench/store';
import { findTab } from '@/features/workbench/layout';
import { useQuery } from '@tanstack/react-query';
import { useRouter, useRouterState } from '@tanstack/react-router';
import { MessageSquareOff, Maximize2 } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { useTranslation } from 'react-i18next';

import { useViewChoice } from '@/lib/viewChoices';
import { nodeChatOpeningQuery } from '@/api/chats';
import { ApiError } from '@/api/client';
import { ChatView, type ChatViewProps } from '@/features/chat/ChatView';
import { useTakeQuotes } from '@/features/chat/discuss';
import { clearChatFocus, useComposerFocus, useRequestedChat } from '@/features/chat/focus';
import { type Segment } from '@/features/chat/panel';
import { useViewShown } from '@/features/chat/shown';
import type { ViewProps } from '@/features/workbench/views';
import { useChatUnread } from '@/realtime/stores/chat';
import { useCurrentHeading } from '@/shell/screen';
import { Badge } from '@/ui/Badge';
import { Drawer } from '@/ui/Drawer';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { SegmentedControl } from '@/ui/SegmentedControl';
import { SkeletonRows } from '@/ui/Skeleton';

/** What the address says about the panel: that it is wanted, which chat, and which message. */
function useChatLink(): { panel: boolean; chat: string | null; message: string | null } {
  const routeSearch = useRouterState({ select: (state) => state.location.search }) as Record<string, unknown>;
  const instance = useViewInstance();
  const tab = useWorkbench((state) => instance === null ? null : findTab(state.layout, instance.id)?.tab ?? null);
  const search: Record<string, unknown> = tab?.scope?.mode === 'fixed' ? tab.of ?? {} : routeSearch;
  const text = (value: unknown): string | null => (typeof value === 'string' && value !== '' ? value : null);
  return { panel: search.panel === 'chat', chat: text(search.chat), message: text(search.message) };
}

function SegmentLabel({ segment }: { segment: Segment }) {
  const { t } = useTranslation();
  const { unread, mentions } = useChatUnread(segment.chatId, segment.seq);
  const kind = t(`chat.segment.${segment.kind}`);
  return (
    <span className="inline-flex min-w-0 items-center gap-1">
      <span className="min-w-0 truncate">{kind}</span>
      {mentions > 0 ? (
        <Badge tone="accent" count={mentions} label={t('chat.unread.mentions', { count: mentions })} />
      ) : (
        unread && (
          <>
            <span aria-hidden className="size-2 rounded-full bg-accent-solid" />
            <span className="sr-only">{t('chat.unread.dot')}</span>
          </>
        )
      )}
    </span>
  );
}

/** Short scope names in the chat's one header row; each full place name stays in its tooltip. */
export function ChatScope({ segments, value, onValueChange, restoreFocusRef }: { segments: readonly Segment[]; value: string; onValueChange: (id: string) => void; restoreFocusRef?: RefObject<boolean> }) {
  const { t } = useTranslation();
  const group = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (restoreFocusRef?.current !== true) return;
    // Changing chats replaces the live conversation, but the keyboard stays in its scope switch.
    const selected = group.current?.querySelector<HTMLButtonElement>('[role="radio"][data-state="checked"]');
    if (selected) { selected.focus(); restoreFocusRef.current = false; }
  }, [value, restoreFocusRef]);
  return <div ref={group} className="min-w-0 shrink-0">
    <SegmentedControl label={t('chat.segments')} size="sm" value={value} onValueChange={onValueChange}
      className="[&_button]:px-1 [&_[role=radiogroup]]:flex-nowrap"
      segments={segments.map((segment) => ({ value: segment.chatId, name: <SegmentLabel segment={segment} />, tooltip: segment.name }))} />
  </div>;
}

/**
 * The panel admits its scopes and selected conversation together; cached path names never grant a chat.
 */
function useSegments(nodeId: string, shown: boolean, scope: string, chatId: string | null): { segments: Segment[]; failed: unknown; empty: boolean; retry: () => void } {
  const opening = useQuery({ ...nodeChatOpeningQuery(nodeId, scope, chatId), enabled: shown });
  const refused = opening.isError && opening.error instanceof ApiError && [403, 404].includes(opening.error.status);
  const data = refused ? undefined : opening.data;
  const segments = useMemo(() => (data?.scopes ?? []).flatMap(({ node, chat }) => node.name === null || node.redacted ? [] : [{
    id: node.id, kind: node.kind, name: node.name, chatId: chat.id, seq: chat.seq,
  }]), [data]);
  return { segments, failed: opening.isError ? opening.error : null, empty: opening.isSuccess && segments.length === 0, retry: () => void opening.refetch() };
}

/** The view of one chat in the panel: what "Discuss in chat" left for it stands in its box. */
function PanelChat(props: Omit<ChatViewProps, 'onReady'>) {
  const takeQuotes = useTakeQuotes(props.chatId);
  const onReady = useComposerFocus(props.chatId, takeQuotes);
  return <ChatView {...props} onReady={onReady} />;
}

function PanelOf({ nodeId, shown, named }: { nodeId: string; shown: boolean; named: boolean }) {
  const { t } = useTranslation();
  const link = useChatLink();
  const [scope, chooseScope] = useViewChoice<string>('chat', 'scope', 'thread');
  const requestedChat = useRequestedChat();
  const { segments, failed, empty, retry } = useSegments(nodeId, shown, scope, requestedChat ?? link.chat);
  const [expanded, setExpanded] = useState(false);
  const scopeFocus = useRef(false);
  const router = useRouter();
  const requested = segments.find((segment) => segment.chatId === requestedChat);
  const clearLink = useCallback(() => {
    const url = new URL(router.state.location.href, window.location.origin);
    if (!url.searchParams.has('chat') && !url.searchParams.has('message')) return;
    url.searchParams.delete('chat'); url.searchParams.delete('message');
    if (url.searchParams.get('panel') === 'chat') url.searchParams.delete('panel');
    router.history.replace(url.pathname + url.search + url.hash);
  }, [router]);
  const requestedKind = requested?.kind;
  useEffect(() => {
    if (requestedKind === undefined) return;
    if (scope !== requestedKind) chooseScope(requestedKind);
    clearLink();
  }, [requestedKind, scope, chooseScope, clearLink]);

  if (segments.length === 0) {
    if (empty) return <EmptyState compact className="h-full" icon={<MessageSquareOff aria-hidden />} title={t('chat.panel.none')} />;
    return failed === null ? (
      <SkeletonRows rows={6} className="p-4" />
    ) : (
      <ErrorState compact className="h-full" error={failed} title={t('chat.couldNotLoad')} onRetry={retry} />
    );
  }
  const chosen = requested ?? segments.find((segment) => segment.chatId === link.chat) ?? segments.find((segment) => segment.kind === scope) ?? segments[0];
  if (chosen === undefined) return null;
  const chatId = chosen.chatId;
  const several = segments.length > 1;

  const expand = (
    <IconButton size="sm" label={t('chat.expand')} icon={<Maximize2 aria-hidden />} onClick={() => setExpanded(true)} />
  );
  const scopes = several ? <ChatScope segments={segments} value={chatId} restoreFocusRef={scopeFocus} onValueChange={(id) => { const target = segments.find((segment) => segment.chatId === id); if (target !== undefined) { clearChatFocus(); scopeFocus.current = true; chooseScope(target.kind); clearLink(); } }} /> : undefined;
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="min-h-0 flex-1">
        {expanded ? (
          <EmptyState compact className="h-full" icon={<Maximize2 aria-hidden />} title={t('chat.expanded')} />
        ) : (
          <PanelChat
            key={chatId}
            chatId={chatId}
            shown={shown}
            message={link.chat === chatId || link.chat === null ? link.message : null}
            heading={!several && named ? 'name' : 'none'}
            actions={expand}
            scope={scopes}
          />
        )}
      </div>
      <Drawer
        side="right"
        open={expanded}
        onOpenChange={setExpanded}
        title={t('chat.title.node', { name: chosen.name, kind: t(`chat.kind.${chosen.kind}`) })}
        className="[--right-bar-width:80vw]"
      >
        {expanded && <PanelChat key={`expanded-${chatId}`} chatId={chatId} shown heading="none" scope={scopes} />}
      </Drawer>
    </div>
  );
}

export function ChatPanel({ tab, area }: ViewProps) {
  const { t } = useTranslation();
  const heading = useCurrentHeading();
  const shown = useViewShown(tab, area);
  // Following a document does not grant its parent chat. An explicitly named chat owns its own read.
  const nodeId = tab.of?.node ?? (heading?.views?.chat === false ? null : heading?.nodeId ?? null);
  if (nodeId === null) {
    return <EmptyState compact className="h-full" icon={<MessageSquareOff aria-hidden />} title={t('chat.panel.none')} description={t('chat.panel.noneAbout')} />;
  }
  return <PanelOf key={nodeId} nodeId={nodeId} shown={shown} named={tab.of?.node !== undefined && heading?.nodeId !== nodeId} />;
}
