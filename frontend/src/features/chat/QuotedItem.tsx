/** A timeline item quoted into a chat: a card with the item's kind, author, time and first lines, which leads to the item in the timeline.
 *
 * Quoting stays tied to its item: the card is read from the thread, so it shows the item as it is,
 * and pressing it goes to the thread with that item lit. A reader who may not read the timeline is
 * shown that an item was quoted and nothing of it.
 */
import { useQuery } from '@tanstack/react-query';
import { MessageSquareQuote, X } from 'lucide-react';
import { useTranslation } from 'react-i18next';

import { quotedItemQuery } from '@/api/chats';
import { isPlainClick, useOpenPath } from '@/features/chat/open';
import type { Item } from '@/generated/api-types';
import { IconButton } from '@/ui/IconButton';
import { RelativeTime } from '@/ui/RelativeTime';
import { Skeleton } from '@/ui/Skeleton';
import { UserChip } from '@/ui/UserChip';

/** How many characters of an item the card shows: a few lines, not the item. */
const EXCERPT_CHARS = 280;

/** What an item says, as plain text: what a person or the agent wrote; nothing for a step that is no text. */
export function textOfItem(item: Item): string {
  if (item.kind === 'user_message') {
    return item.payload.parts
      // A copy that was forwarded into the thread says its text as well; the files and images are not quoted.
      .map((part) => ('source_id' in part ? part.text : part.type === 'text' ? part.text : ''))
      .filter((text) => text !== '')
      .join('\n');
  }
  if (item.kind === 'agent_message') return item.payload.text;
  return '';
}

/** The path of an item in the thread's timeline, as a link carries it. */
export function itemPath(threadId: string, itemId: string, continuation?: string | null): string {
  return `/thread/${threadId}?item=${itemId}${continuation === undefined || continuation === null ? '' : `&continuation=${encodeURIComponent(continuation)}`}`;
}

export interface QuotedItemProps {
  threadId: string;
  itemId: string;
  /** Present in a composer, where the quote can be taken out again. */
  onRemove?: () => void;
}

export function QuotedItem({ threadId, itemId, onRemove }: QuotedItemProps) {
  const { t } = useTranslation();
  const open = useOpenPath();
  const item = useQuery(quotedItemQuery(threadId, itemId));
  const path = itemPath(threadId, itemId, item.data?.continuation_id);
  const body =
    item.isPending ? (
      <Skeleton className="h-10 w-full" />
    ) : item.isError ? (
      // The reader may not read this timeline, or the item is gone: they are told an item was quoted, and not what it said.
      <span className="text-sm text-text-muted">{t('chat.quote.unreadable')}</span>
    ) : (
      <>
        <span className="flex flex-wrap items-center gap-x-2 text-xs text-text-muted">
          <span className="font-medium text-text">{t(`chat.quote.kind.${item.data.kind}`)}</span>
          <RelativeTime at={item.data.created_at} />
        </span>
        {textOfItem(item.data) !== '' && (
          <span data-overflow-text={textOfItem(item.data)} className="line-clamp-3 text-sm wrap-anywhere text-text-muted">{Array.from(textOfItem(item.data)).slice(0, EXCERPT_CHARS).join('')}</span>
        )}
      </>
    );
  return (
    <span className="flex max-w-full items-start gap-1 rounded-md border border-border bg-bg-subtle">
      {item.isError ? (
        <span className="flex min-w-0 flex-1 items-start gap-2 px-2 py-2">
          <MessageSquareQuote size={16} aria-hidden className="mt-0.5 shrink-0 text-text-subtle" />
          <span className="flex min-w-0 flex-col gap-1">{body}</span>
        </span>
      ) : (
        <span className="flex min-w-0 flex-1 flex-col">
          {item.isSuccess && item.data.author !== null && <span className="px-2 pt-2"><UserChip user={item.data.author} size={24} thing={{ kind: 'thread', id: threadId, ...(item.data.turn_id === null ? {} : { anchor: { turn: item.data.turn_id } }) }} /></span>}
        <a
          href={path}
          aria-label={t('chat.quote.open')}
          onClick={(event) => {
            if (!isPlainClick(event)) return;
            event.preventDefault();
            open(path);
          }}
          className="focus-inset flex min-w-0 flex-1 items-start gap-2 rounded-md px-2 py-2 hover:bg-bg-muted"
        >
          <MessageSquareQuote size={16} aria-hidden className="mt-0.5 shrink-0 text-accent-text" />
          <span className="flex min-w-0 flex-col gap-1">{body}</span>
        </a>
        </span>
      )}
      {onRemove !== undefined && <IconButton size="sm" label={t('chat.quote.remove')} icon={<X aria-hidden />} onClick={onRemove} />}
    </span>
  );
}
