/** Pinned messages.
 *
 * A button in the chat's header that opens the messages somebody pinned, oldest first, each with its
 * author and first line; pressing one goes to it in the conversation. The list is read when it opens,
 * so it is the chat's pins as they are now, not as they were when the chat was opened.
 */
import { useQuery } from '@tanstack/react-query';
import { Pin } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { pinnedQuery } from '@/api/chats';
import { firstLine, plain, textOfParts } from '@/features/chat/text';
import { IconButton } from '@/ui/IconButton';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

/** How many characters of a pinned message the list shows. */
const LINE_CHARS = 100;

export function Pinned({ chatId, onJump }: { chatId: string; onJump: (messageId: string) => void }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const pinned = useQuery({ ...pinnedQuery(chatId), enabled: open, staleTime: 0 });
  return (
    <Popover
      label={t('chat.pinned.label')}
      scroll="children"
      open={open}
      onOpenChange={setOpen}
      align="end"
      className="w-(--menu-max-width) max-w-[90vw] p-2"
      trigger={<IconButton size="sm" label={t('chat.pinned.label')} icon={<Pin aria-hidden />} />}
    >
      <div className="flex min-h-0 flex-col gap-1">
        <p className="px-2 py-1 text-sm font-medium text-text">{t('chat.pinned.label')}</p>
        {pinned.isPending ? (
          <Skeleton className="h-10 w-full" />
        ) : pinned.isError ? (
          <ErrorState compact error={pinned.error} onRetry={() => void pinned.refetch()} />
        ) : pinned.data.length === 0 ? (
          <EmptyState compact className="px-2" title={t('chat.pinned.none')} />
        ) : (
          <ul className="flex min-h-0 max-h-[40dvh] flex-col overflow-y-auto">
            {pinned.data.map((message) => (
              <li key={message.id} className="flex flex-col gap-1 rounded-md px-2 py-1 hover:bg-bg-muted">
                <span className="flex items-center gap-2 text-xs text-text-muted">
                  {message.author !== null && 'id' in message.author ? <UserChip user={message.author} size={24} thing={{ kind: 'chat', id: chatId, anchor: { message: message.id } }} /> : <span className="font-medium text-text">{message.author?.agent ?? ''}</span>}
                  <RelativeTime at={message.created_at} />
                </span>
                <button
                  type="button"
                  aria-label={`${t('chat.reply.open')} · ${firstLine(message.parts, LINE_CHARS)}`}
                  onClick={() => {
                    setOpen(false);
                    onJump(message.id);
                  }}
                  className="focus-inset w-full rounded-md text-left hover:text-accent-text"
                >
                  <span data-overflow-text={plain(textOfParts(message.parts))} className="truncate text-sm text-text">{firstLine(message.parts, LINE_CHARS)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </Popover>
  );
}
