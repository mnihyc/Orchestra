/** Export… in the menu of a chat: the entries a chat's header menu adds, and the menu of a chat that has no other.
 *
 * A chat of a node is taken away by whoever holds its Export row on that node; a direct or a group chat by the
 * people in it, and not by an admin who only reads it as one. Where the caller may not, the entry stays,
 * switched off, with the line that says for whom it is and, for a node's chat, Request access.
 */
import { useQuery } from '@tanstack/react-query';
import { EllipsisVertical } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { nodeQuery } from '@/api/nodes';
import type { Chat } from '@/generated/api-types';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';

import { openExport, requestExportAccess } from './ExportHost';
import { exportEntries } from './menu';
import { mayExportChat } from './pending';
import type { ExportTarget } from './target';

/** What a chat is called in the words of its menu and its dialog: its place, its name, or the people in it. */
export function chatLabel(chat: Chat, me: string | null): string {
  if (chat.kind === 'node') return chat.node?.name ?? '';
  const others = (chat.members ?? []).filter((member) => member.id !== me).map((member) => member.display_name);
  return chat.name ?? others.join(', ');
}

/** The entries of Export… for a chat, ready to stand in a menu. */
export function useChatExportEntries(chat: Chat, me: string | null): MenuEntry[] {
  const { t } = useTranslation();
  // The chat already carries this reader's export decision. An owner's cached name can enrich
  // the request, but opening a chat or its menu never needs a second read of the place.
  const node = useQuery({ ...nodeQuery(chat.node?.id ?? ''), enabled: false });
  const allowed = mayExportChat(chat);
  const target: ExportTarget = {
    kind: 'chat',
    id: chat.id,
    name: chatLabel(chat, me),
    owner: node.data?.owner ?? null,
    ...(chat.node === null ? {} : { page: { kind: chat.node.kind, id: chat.node.id } }),
  };
  return exportEntries({
    allowed,
    owner: node.data?.owner ?? null,
    where: 'screen',
    ...(chat.kind === 'node' ? {} : { reason: t('export.menu.onlyThePeople') }),
    onExport: () => openExport(target),
    onRequest: () => requestExportAccess(target),
    t,
  });
}

/** The menu of the header of a chat that has none of its own: a chat of a node and a direct chat. */
export function ChatMenu({ chat, me }: { chat: Chat; me: string | null }) {
  const { t } = useTranslation();
  const entries = useChatExportEntries(chat, me);
  if (entries.length === 0) return null;
  return <Menu label={t('export.chatMenu')} align="end" entries={entries} trigger={<IconButton size="sm" label={t('export.chatMenu')} icon={<EllipsisVertical aria-hidden />} />} />;
}
