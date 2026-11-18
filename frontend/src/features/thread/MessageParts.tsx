/** The parts of moving between the people's messages that are drawn: the two arrows, the sticky prompt and the Messages list. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { outlineQuery } from '@/api/threads';
import type { MessageNav } from '@/features/thread/messageMoves';
import type { PersonMessage } from '@/features/thread/messages';
import { useThreadView } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useDebounced } from '@/lib/debounce';
import { useFormatContext } from '@/lib/appearance';
import { formatNumber } from '@/lib/format';
import { ErrorState } from '@/ui/ErrorState';
import { Pagination } from '@/ui/Pagination';
import { SkeletonRows } from '@/ui/Skeleton';
import { Tooltip } from '@/ui/Tooltip';
import { IconButton } from '@/ui/IconButton';
import { Input } from '@/ui/Input';
import { RelativeTime } from '@/ui/RelativeTime';
import { UserChip } from '@/ui/UserChip';

/** The previous and next arrows: in the header row and on the sticky prompt. */
export function MessageArrows({ nav }: { nav: MessageNav }) {
  const { t } = useTranslation();
  return (
    <span className="inline-flex shrink-0 items-center" role="group" aria-label={t('thread.messages.arrows')}>
      <IconButton size="sm" label={t('thread.messages.previous')} icon={<ChevronUp aria-hidden />} shortcut="Mod+Alt+ArrowUp" disabled={!nav.canPrevious} onClick={() => nav.previous()} />
      <IconButton size="sm" label={t('thread.messages.next')} icon={<ChevronDown aria-hidden />} shortcut="Mod+Alt+ArrowDown" disabled={!nav.canNext} onClick={() => nav.next()} />
    </span>
  );
}

/** One dim line under the head, while the message that began the turn in view has scrolled above it: who, what, where among the messages, and the arrows. */
export function StickyPrompt({ nav }: { nav: MessageNav }) {
  const { t } = useTranslation();
  const message = nav.pinned;
  if (message === null) return null;
  return (
    <div data-sticky-prompt className="absolute inset-x-0 top-0 z-(--layer-sticky) flex min-h-(--control-sm) items-center gap-1 border-b border-border bg-surface px-4 text-sm text-text-muted">
      <button type="button" aria-label={t('thread.messages.goTo', { line: message.line })} onClick={() => nav.go(message.id)}
        className="focus-inset flex min-w-0 flex-1 items-center gap-2 rounded-md py-0.5 text-left hover:text-text">
        <UserChip user={message.author} size={24} card={false} nameClassName="text-text-muted" />
        <span className="min-w-0 flex-1 truncate">{message.line}</span>
      </button>
      {nav.place !== null && <span className="shrink-0 text-xs tabular-nums">{t('thread.messages.place', { at: nav.place, total: nav.messages.length })}</span>}
      <MessageArrows nav={nav} />
    </div>
  );
}

/** A row of the Messages list: a message held by the page, or an older one that only the thread's outline knows. */
interface Row {
  id: string;
  seq: number;
  at: string;
  line: string;
  text: string;
  author: PersonMessage['author'] | null;
  added: number | null;
  removed: number | null;
}

/** The people's messages only: who (where more than one person wrote), when, the first line and what the turn changed; searchable, with its own scroll. */
export function MessagesPanel({ threadId, nav, onGo }: { threadId: string; nav: MessageNav; onGo: (id: string) => void }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(threadId);
  const [asked, setAsked] = useState('');
  const { locale } = useFormatContext();
  const wanted = asked.trim().toLocaleLowerCase();
  const q = useDebounced(wanted, 250);
  const remote = view.hasOlder || view.hasNewer;
  const options = outlineQuery(read, { selection: 'messages', q });
  const outline = useInfiniteQuery({ ...options, enabled: remote && options.enabled, staleTime: Infinity, refetchOnWindowFocus: false, refetchOnReconnect: false, retry: false });
  const rows = useMemo<Row[]>(() => {
    const stored = new Map((remote ? outline.data?.pages.flatMap((page) => page.entries) ?? [] : []).flatMap((entry) => {
      if (entry.kind !== 'user_message' || entry.message?.source !== 'user') return [];
      return [[entry.item_id, { id: entry.item_id, seq: entry.seq, at: entry.at, line: entry.title, text: entry.title,
        author: entry.message.author ?? null, added: entry.message.added ?? null, removed: entry.message.removed ?? null }] as const];
    }));
    // The server searched the complete sanitized text and sender before paging. Its matching title need not repeat the words.
    for (const message of nav.messages) {
      if (stored.has(message.id)) continue;
      if (wanted !== '' && !message.text.toLocaleLowerCase().includes(wanted) && !message.author.display_name.toLocaleLowerCase().includes(wanted) && !message.author.username.toLocaleLowerCase().includes(wanted)) continue;
      stored.set(message.id, { ...message, added: remote ? null : message.added, removed: remote ? null : message.removed });
    }
    return [...stored.values()].sort((a, b) => a.seq - b.seq);
  }, [nav.messages, outline.data, remote, wanted]);
  const looking = remote && (outline.isPending || q !== wanted);
  const several = remote || new Set(nav.messages.map((message) => message.author.id)).size > 1;

  return (
    <div className="flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-2" data-messages-panel>
      <Input label={t('thread.messages.search')} hideLabel value={asked} placeholder={t('thread.messages.search')} onChange={(event) => setAsked(event.target.value)} />
      <div className="max-h-80 min-h-0 overflow-y-auto" aria-busy={looking || outline.isFetchingNextPage}>
        {looking && <div role="status" aria-label={t('thread.messages.loading')}><SkeletonRows rows={2} /></div>}
        {!looking && rows.length === 0 && !outline.isError && !outline.hasNextPage && <p className="px-1 py-2 text-sm text-text-muted">{t(wanted === '' ? 'thread.messages.none' : 'thread.messages.noMatch')}</p>}
        <ul aria-label={t('thread.messages.list')} className="flex min-h-0 flex-col">
          {(!looking || q === wanted) && rows.map((row) => (
            <li key={row.id} className="flex min-w-0 items-center gap-2 px-1">
              {several && row.author !== null && <UserChip user={row.author} className="max-w-24 shrink-0" />}
              <button type="button" onClick={() => onGo(row.id)} className="focus-inset flex min-w-0 flex-1 items-baseline gap-2 rounded-md px-1 py-1 text-left text-sm hover:bg-bg-muted">
                <RelativeTime at={row.at} compact className="text-xs text-text-muted" />
                <span className="min-w-0 flex-1 truncate text-text">{row.line}</span>
                {row.added === null || row.removed === null ? <Tooltip content={t('thread.messages.changesUnknown')}><span className="text-xs text-text-subtle">–</span></Tooltip>
                  : row.added + row.removed > 0 && <span className="shrink-0 text-xs"><span className="text-success-text">{`+${formatNumber(row.added, locale)}`}</span> <span className="text-danger-text">{`−${formatNumber(row.removed, locale)}`}</span></span>}
              </button>
            </li>
          ))}
        </ul>
        {remote && outline.isError && outline.data === undefined ? <ErrorState compact error={outline.error} onRetry={() => void outline.refetch()} />
          : remote && <Pagination hasMore={outline.hasNextPage} loading={outline.isFetchingNextPage} error={outline.isFetchNextPageError ? outline.error : null} onMore={() => void outline.fetchNextPage()} autoLoad />}
      </div>
    </div>
  );
}
