/** "Discuss in chat": opens the thread's own chat with items of its conversation quoted in the composer.
 *
 * What joins a conversation with an agent to the chat among people is explicit. From an item, or from
 * several selected, the timeline calls this: the Chat panel comes to the front on the thread's chat and
 * the items stand in its composer as quotes that stay tied to them. Nothing is sent until the person sends it.
 */
import { useCallback, useEffect, useRef } from 'react';
import { create } from 'zustand';
import { client } from '@/api/client';

import type { ChatViewProps } from '@/features/chat/ChatView';
import type { Quote } from '@/features/chat/draft';
import { revealView } from '@/features/workbench/openView';
import { useTreeStore } from '@/realtime/stores/tree';
import { currentHeading } from '@/shell/screen';
import { requestChatFocus } from '@/features/chat/focus';
import { threadViewOf } from '@/realtime/stores/thread';

interface Pending {
  /** The quotes waiting for the composer of each chat. */
  quotes: ReadonlyMap<string, readonly Quote[]>;
}

export const useDiscuss = create<Pending>(() => ({ quotes: new Map() }));
client.onSession((event) => { if (event.type === 'signed_out') useDiscuss.setState({ quotes: new Map() }); });

/** The chat of a node, from the tree the page holds. */
export function chatOfNode(nodeId: string): string | null {
  const thread = threadViewOf(nodeId);
  if (thread.closed === null && thread.thread !== null) return thread.thread.chat_id;
  for (const tree of useTreeStore.getState().byOrg.values()) {
    const found = tree.nodes.get(nodeId);
    if (found !== undefined) return found.chat.id;
  }
  return null;
}

/** Takes what waits for a chat's composer, leaving nothing behind. */
export function takeQuotes(chatId: string): readonly Quote[] {
  const waiting = useDiscuss.getState().quotes.get(chatId) ?? [];
  if (waiting.length > 0) {
    useDiscuss.setState((state) => {
      const quotes = new Map(state.quotes);
      quotes.delete(chatId);
      return { quotes };
    });
  }
  return waiting;
}

/**
 * For the view of a chat: what "Discuss in chat" left for this chat goes into its box, at once if the box is there and as soon as
 * it is if not, and what is left later goes in as it comes. Hand the result to the view as `onReady`.
 */
export function useTakeQuotes(chatId: string): NonNullable<ChatViewProps['onReady']> {
  const box = useRef<Parameters<NonNullable<ChatViewProps['onReady']>>[0] | null>(null);
  const give = useCallback(() => {
    const api = box.current;
    if (api === null) return;
    const waiting = takeQuotes(chatId);
    if (waiting.length === 0) return;
    api.quote(waiting);
    api.focus();
  }, [chatId]);
  useEffect(() => useDiscuss.subscribe(give), [give]);
  return useCallback(
    (api) => {
      box.current = api;
      give();
    },
    [give],
  );
}

/** Brings the Chat view to the front, on the right. */
export function showChat(): void {
  const node = currentHeading()?.nodeId;
  revealView('chat', node === undefined ? undefined : { node });
}

/**
 * Quotes items of a thread into its chat and brings the chat to the front. Answers whether the chat is
 * known (the thread is in a tree the page holds); the caller says so if it is not.
 */
export function discussInChat(threadId: string, itemIds: readonly string[], route?: string): boolean {
  const chatId = chatOfNode(threadId);
  if (chatId === null || itemIds.length === 0) return false;
  useDiscuss.setState((state) => {
    const quotes = new Map(state.quotes);
    quotes.set(chatId, [...(quotes.get(chatId) ?? []), ...itemIds.map((itemId) => ({ threadId, itemId }))]);
    return { quotes };
  });
  requestChatFocus(chatId, route);
  revealView('chat', { node: threadId });
  return true;
}
