/** The parts of moving between the people's messages that are drawn: the two arrows, the sticky prompt and the Messages list. */
import { useInfiniteQuery } from '@tanstack/react-query';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { outlineQuery } from '@/api/threads';
import type { MessageNav } from '@/features/thread/messageMoves';
import type { PersonMessage } from '@/features/thread/messages';
import { outlineRows } from '@/features/inspector/outlineRows';
import { useThreadView } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
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
  at: string;
  line: string;
  text: string;
  author: PersonMessage['author'] | null;
  added: number;
  removed: number;
}

/** The people's messages only: who (where more than one person wrote), when, the first line and what the turn changed; searchable, with its own scroll. */
export function MessagesPanel({ threadId, nav, onGo }: { threadId: string; nav: MessageNav; onGo: (id: string) => void }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(threadId);
  const view = useThreadView(threadId);
  const [asked, setAsked] = useState('');
  // The messages older than the page holds are known to the outline of the thread; it is read only while there are any.
  const options = outlineQuery(read);
  const outline = useInfiniteQuery({ ...options, enabled: view.hasOlder && options.enabled, staleTime: Infinity, refetchOnWindowFocus: false, refetchOnReconnect: false });
  const rows = useMemo<Row[]>(() => {
    const held = new Set(nav.messages.map((message) => message.id));
    const older: Row[] = view.hasOlder ? outlineRows(outline.data?.pages.flatMap((page) => page.entries) ?? [], view).flatMap((entry) =>
      entry.kind === 'user_message' && typeof entry.goal_condition !== 'string' && !held.has(entry.item_id)
        ? [{ id: entry.item_id, at: entry.at, line: entry.title, text: entry.title, author: null, added: 0, removed: 0 }] : []) : [];
    return [...older, ...nav.messages];
  }, [nav.messages, outline.data, view]);
  const several = new Set(nav.messages.map((message) => message.author.id)).size > 1;
  const wanted = asked.trim().toLowerCase();
  const shown = wanted === '' ? rows : rows.filter((row) => row.text.toLowerCase().includes(wanted) || (row.author?.display_name.toLowerCase().includes(wanted) ?? false));
  return (
    <div className="flex w-96 max-w-[calc(100vw-2rem)] flex-col gap-2" data-messages-panel>
      <Input label={t('thread.messages.search')} hideLabel value={asked} placeholder={t('thread.messages.search')} onChange={(event) => setAsked(event.target.value)} />
      {shown.length === 0 ? <p className="px-1 py-2 text-sm text-text-muted">{t(rows.length === 0 ? 'thread.messages.none' : 'thread.messages.noMatch')}</p> : (
        <ul aria-label={t('thread.messages.list')} className="flex max-h-80 min-h-0 flex-col overflow-y-auto">
          {shown.map((row) => (
            <li key={row.id}>
              <button type="button" onClick={() => onGo(row.id)} className="focus-inset flex w-full items-baseline gap-2 rounded-md px-2 py-1 text-left text-sm hover:bg-bg-muted">
                {several && row.author !== null && <span className="max-w-24 shrink-0 truncate font-medium text-text">{row.author.display_name}</span>}
                <RelativeTime at={row.at} compact className="text-xs text-text-muted" />
                <span className="min-w-0 flex-1 truncate text-text">{row.line}</span>
                {row.added + row.removed > 0 && <span className="shrink-0 text-xs"><span className="text-success-text">{`+${row.added}`}</span> <span className="text-danger-text">{`−${row.removed}`}</span></span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
