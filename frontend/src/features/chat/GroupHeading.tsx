/** Public discovery never joins: only the native Join fact offers the explicit act. */
import { useIsMutating, useMutation, useQueryClient } from '@tanstack/react-query';
import { useStore } from 'zustand';
import { useTranslation } from 'react-i18next';
import { client } from '@/api/client';
import { chatKey, joinGroup, orgChatsKey } from '@/api/chats';
import type { Chat } from '@/api/chat-contract';
import { libraryReader, sameLibraryReader } from '@/api/resourceLibrary';
import { GroupAccessSummary } from '@/features/access/GroupAccess';
import { refreshChat } from '@/realtime/stores/chat';
import { Button } from '@/ui/Button';
import { ErrorAlert } from '@/ui/ErrorState';

export function GroupHeading({ chat }: { chat: Chat }) {
  const { t } = useTranslation();
  const cache = useQueryClient();
  const me = useStore(client.session, (state) => state.userId);
  const mutationKey = ['group-join', chat.id, JSON.stringify(libraryReader())] as const;
  const pending = useIsMutating({ mutationKey }) > 0;
  const join = useMutation({ mutationKey, mutationFn: (asked: { id: string; reader: ReturnType<typeof libraryReader> }) => joinGroup(asked.id),
    onSuccess: (_reply, asked) => {
      if (!sameLibraryReader(asked.reader)) return;
      refreshChat(asked.id);
      void cache.invalidateQueries({ queryKey: chatKey(asked.id), exact: true });
      void cache.invalidateQueries({ queryKey: orgChatsKey(chat.org_id) });
    } });
  const thisReader = join.variables?.id === chat.id && sameLibraryReader(join.variables.reader);
  return <div data-group-heading className="flex min-w-0 shrink-0 flex-wrap items-center gap-2 border-b border-border px-3 py-1">
    <div className="min-w-0 flex-1"><GroupAccessSummary chat={chat} me={me} /></div>
    {chat.can.join === true && <Button size="sm" pending={pending} onClick={() => {
      if (!pending) join.mutate({ id: chat.id, reader: libraryReader() });
    }}>{t('chat.group.join')}</Button>}
    {thisReader && join.error !== null && <ErrorAlert error={join.error} />}
  </div>;
}
