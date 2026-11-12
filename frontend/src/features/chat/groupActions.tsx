import { groupHomeMoveEntries } from '@/features/duplicate/move';
import { duplicateEntries } from '@/features/duplicate/target';
/** Group entry actions use actual native eligibility and the existing Share entrance. */
import { useIsMutating, useMutation, useQueryClient } from '@tanstack/react-query';
import { MessageSquare, Share2, UserPlus } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { chatKey, joinGroup, orgChatsKey } from '@/api/chats';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { openAccess } from '@/features/access/AccessDialog';
import { linkTo } from '@/lib/links';
import { refreshChat } from '@/realtime/stores/chat';
import type { MenuEntry } from '@/ui/Menu';
import { toast } from '@/ui/Toast';
import { useErrorText } from '@/ui/errorText';

export interface GroupActionTarget { id: string; name: string | null; org_id?: string; can?: { join?: boolean; manage?: boolean } }
export function useGroupActions(chat: GroupActionTarget | null, open: (href: string) => void): { entries: MenuEntry[]; pending: boolean } {
  const { t } = useTranslation();
  const describe = useErrorText();
  const cache = useQueryClient();
  const mutationKey = ['group-join', chat?.id, JSON.stringify(libraryReader())] as const;
  const pending = useIsMutating({ mutationKey }) > 0;
  const join = useMutation({ mutationKey, mutationFn: (asked: { id: string; org?: string; reader: ReturnType<typeof libraryReader> }) => joinGroup(asked.id),
    onSuccess: (_reply, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      refreshChat(asked.id);
      void cache.invalidateQueries({ queryKey: chatKey(asked.id), exact: true });
      void cache.invalidateQueries({ queryKey: asked.org === undefined ? ['chats'] : orgChatsKey(asked.org) });
      toast({ tone: 'success', title: t('chat.group.joined') });
    }, onError: (error, asked) => { if (sameLibraryReader(asked.reader)) toast({ tone: 'danger', title: t('chat.group.joinFailed'), description: describe(error).message }); } });
  if (chat === null) return { entries: [], pending: false };
  const name = chat.name ?? t('chat.title.groupUnnamed');
  return { pending, entries: [
    { id: 'message', label: t('chat.group.message'), icon: <MessageSquare aria-hidden />, onSelect: () => open(linkTo({ kind: 'chat', id: chat.id })) },
    ...(chat.can?.join !== true ? [] : [{ id: 'join', label: t('chat.group.join'), icon: <UserPlus aria-hidden />, disabled: pending ? t('chat.group.joining') : false,
      onSelect: () => { if (!pending) join.mutate({ id: chat.id, org: chat.org_id, reader: libraryReader() }); } } as MenuEntry]),
    ...duplicateEntries(t, { kind: 'chat', id: chat.id, name }),
    ...groupHomeMoveEntries(t, chat),
    { id: 'share', label: t('everything.share'), icon: <Share2 aria-hidden />, onSelect: () => openAccess({ kind: 'chat', id: chat.id, name }) },
  ] };
}
