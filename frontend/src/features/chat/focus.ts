/** A navigation shortcut hands focus to the one live chat composer when its data is ready. */
import { useCallback, useContext, useEffect, useRef } from 'react';
import { create } from 'zustand';
import { client } from '@/api/client';
import type { ChatViewProps } from './ChatView';
import { ViewRouteContext } from '@/features/workbench/viewOpening';
import { openingId } from '@/lib/opening';

const useChatFocus = create<{ pending: { chatId: string; route: string } | null }>(() => ({ pending: null }));
client.onSession((event) => { if (event.type === 'signed_out') useChatFocus.setState({ pending: null }); });

export function requestChatFocus(chatId: string, route = openingId()): void { useChatFocus.setState({ pending: { chatId, route } }); }

/** Selecting a chat and focusing its composer are one opening intent. */
export function useRequestedChat(): string | null {
  const route = useContext(ViewRouteContext) ?? openingId();
  return useChatFocus((state) => state.pending?.route === route ? state.pending.chatId : null);
}

export function clearChatFocus(): void { useChatFocus.setState({ pending: null }); }

export function useComposerFocus(chatId: string, onReady: NonNullable<ChatViewProps['onReady']>): NonNullable<ChatViewProps['onReady']> {
  const route = useContext(ViewRouteContext) ?? openingId();
  const box = useRef<Parameters<NonNullable<ChatViewProps['onReady']>>[0] | null>(null);
  const give = useCallback(() => {
    const request = useChatFocus.getState().pending;
    if (request?.chatId !== chatId || request.route !== route || box.current === null) return;
    box.current.focus();
    useChatFocus.setState({ pending: null });
  }, [chatId, route]);
  useEffect(() => useChatFocus.subscribe(give), [give]);
  return useCallback((api) => { box.current = api; onReady(api); give(); }, [onReady, give]);
}
