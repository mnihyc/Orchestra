/** A full chat is ordinary independently targeted main content, with its existing reader and composer. */
import { createFileRoute } from '@tanstack/react-router';
import { MessageSquareText } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { ChatView, chatPageTitle } from '@/features/chat/ChatView';
import { usePageParams, usePageSearch } from '@/features/workbench/pageContext';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useChatEntry } from '@/realtime/stores/chat';
import { useScreenHeading } from '@/shell/screen';

interface ChatSearch { message?: string }
export const Route = createFileRoute('/chat/$chatId/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): ChatSearch => typeof search.message === 'string' && search.message !== '' ? { message: search.message } : {},
  component: ChatPage,
});

function ChatPage() {
  const { t } = useTranslation();
  const { chatId } = usePageParams(Route);
  const { message } = usePageSearch(Route);
  const shown = useViewShown();
  const me = useStore(client.session, (state) => state.userId);
  const entry = useChatEntry(chatId);
  const chat = entry.closed === null ? entry.chat : null;
  const title = chat === null ? t('chat.title.groupUnnamed') : chatPageTitle(chat, me, t);
  useScreenHeading({ title, icon: MessageSquareText, views: { ready: chat !== null } });
  return <ChatView chatId={chatId} shown={shown} message={message ?? null} />;
}
