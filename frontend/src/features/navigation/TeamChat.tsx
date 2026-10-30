/** Team chat is a shortcut to the existing workbench view, leaving the page and its draft in place. */
import { MessageSquare } from 'lucide-react';
import { useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { requestChatFocus } from '@/features/chat/focus';
import { LeaveDrawer } from '@/features/navigation/OrgSwitcher';
import { revealView } from '@/features/workbench/openView';
import { ViewRouteContext } from '@/features/workbench/viewOpening';
import { useChatUnread } from '@/realtime/stores/chat';
import { Badge } from '@/ui/Badge';

export function TeamChatShortcut({ nodeId, chatId, seq = 0, className }: { nodeId: string; chatId: string; seq?: number; className?: string }) {
  const { t } = useTranslation();
  const closeNavigation = useContext(LeaveDrawer);
  const route = useContext(ViewRouteContext);
  const { unread, mentions } = useChatUnread(chatId, seq);
  return <button type="button" className={className} onClick={() => {
    requestChatFocus(chatId, route);
    revealView('chat', { node: nodeId }, { focus: false });
    closeNavigation();
  }}>
    <MessageSquare aria-hidden className="shrink-0 text-text-subtle" /><span className="min-w-0 flex-1 truncate text-left">{t('navigation.teamChat')}</span>
    {mentions > 0 ? <Badge tone="accent" count={mentions} label={t('chat.unread.mentions', { count: mentions })} /> : unread && <span aria-label={t('chat.unread.dot')} className="size-2 shrink-0 rounded-full bg-accent-solid" />}
  </button>;
}
