/** A full chat is ordinary independently targeted main content, with its existing reader and composer. */
import { ReferenceFile } from '@/features/files/ReferenceFile';
import { createFileRoute } from '@tanstack/react-router';
import { MessageSquareText } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useStore } from 'zustand';
import { client } from '@/api/client';
import { ChatView, chatPageTitle } from '@/features/chat/ChatView';
import { usePageNavigate, usePageParams, usePageSearch } from '@/features/workbench/pageContext';
import { useViewShown } from '@/features/workbench/viewOpening';
import { useChatEntry } from '@/realtime/stores/chat';
import { useScreenHeading } from '@/shell/screen';

interface ChatSearch { message?: string; file?: string; line?: number; endLine?: number }
export const Route = createFileRoute('/chat/$chatId/')({
  staticData: { screen: 'page' },
  validateSearch: (search: Record<string, unknown>): ChatSearch => {
    const out: ChatSearch = {};
    if (typeof search.message === 'string' && search.message !== '') out.message = search.message;
    if (typeof search.file === 'string' && search.file !== '') out.file = search.file;
    const line = Number(search.line), endLine = Number(search.endLine);
    if (Number.isSafeInteger(line) && line > 0) out.line = line;
    if (out.line !== undefined && Number.isSafeInteger(endLine) && endLine >= out.line) out.endLine = endLine;
    return out;
  },
  component: ChatPage,
});

function ChatPage() {
  const { t } = useTranslation();
  const { chatId } = usePageParams(Route);
  const { message, file, line, endLine } = usePageSearch(Route);
  const navigate = usePageNavigate(Route);
  const shown = useViewShown();
  const me = useStore(client.session, (state) => state.userId);
  const entry = useChatEntry(chatId);
  const chat = entry.closed === null ? entry.chat : null;
  const title = chat === null ? t('chat.title.groupUnnamed') : chatPageTitle(chat, me, t);
  useScreenHeading({ title, icon: MessageSquareText, views: { ready: chat !== null } });
  return <><ChatView chatId={chatId} shown={shown} message={message ?? null} />
    {file !== undefined && <ReferenceFile id={file} chatId={chatId} line={line} endLine={endLine} onClose={() => void navigate({ search: (before) => ({ ...before, file: undefined, line: undefined, endLine: undefined }), replace: true })} />}
  </>;
}
