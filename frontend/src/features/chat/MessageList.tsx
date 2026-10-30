/** Windowed history with day dividers, the line of the new messages, and a jump to the latest.
 *
 * The messages of a chat, oldest first. Only the newest of what is held are drawn, and more as the
 * reader goes up, and older ones are read from the server when the held ones run out, each time
 * keeping what the reader was looking at where it was. The list opens at the line that says where the
 * new messages begin, or at the end; it follows new messages while the reader is within eighty pixels
 * of the end and otherwise says how many came, with a way down. The newest message counts as read two
 * seconds after it has been on screen with the tab visible.
 */
import { ArrowDown, Loader } from 'lucide-react';
import { type ReactNode, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { fetchMessages, MESSAGES_PAGE } from '@/api/chats';
import { useChatContext } from '@/features/chat/context';
import { Message, OutgoingMessage } from '@/features/chat/Message';
import { type ConversationRow, conversationRows, isMine } from '@/features/chat/rows';
import { useAppearance } from '@/lib/appearance';
import { type ChatEntry, type Outgoing, chatWriter, newestHeld, reloadChat } from '@/realtime/stores/chat';
import { Button } from '@/ui/Button';
import { cx } from '@/ui/cx';

/** How near the end the reader must be for the list to follow what arrives: about three lines of text. */
const FOLLOW_WITHIN_PX = 80;
/** How near the top the reader must come for older messages to be asked for: well before the end of what is held. */
const LOAD_OLDER_WITHIN_PX = 240;
/** How many messages are drawn at first, and how many more each time the reader goes up. */
const DRAWN_FIRST = 150;
const DRAWN_MORE = 100;
/** How long the newest message is on screen before it counts as read. */
export const READ_AFTER_MS = 2_000;
/** The line that says where the new messages begin is shown with this much of what came before it above. */
const NEW_LINE_MARGIN_PX = 48;

export interface MessageListProps {
  onRestore?: ((sent: Outgoing) => void) | undefined;
  entry: ChatEntry;
  /** How far the reader had read when the chat was opened. */
  readTo: number | null;
  /** The chat is in front and the tab is visible: only then does anything count as read. */
  watching: boolean;
  /** A message to scroll to, once it is held. */
  target: string | null;
  /** The target was brought into view. */
  onReached: (messageId: string) => void;
  /** The newest message has been on screen long enough to count as read. */
  onRead: (seq: number) => void;
  /** What to show when there is nothing in the chat. */
  empty: ReactNode;
}

/** "Today", "Yesterday" or the date, in the reader's language and time zone. */
function dayLabel(at: string, locale: string, timeZone: string): string {
  const dayOf = (moment: Date) => new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(moment);
  const now = new Date();
  const that = new Date(at);
  const names = new Intl.RelativeTimeFormat(locale, { numeric: 'auto' });
  if (dayOf(that) === dayOf(now)) return names.format(0, 'day');
  if (dayOf(that) === dayOf(new Date(now.getTime() - 24 * 60 * 60_000))) return names.format(-1, 'day');
  const sameYear = new Intl.DateTimeFormat('en', { timeZone, year: 'numeric' }).format(that) === new Intl.DateTimeFormat('en', { timeZone, year: 'numeric' }).format(now);
  return new Intl.DateTimeFormat(locale, { timeZone, weekday: 'long', month: 'long', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) }).format(that);
}

function Divider({ row }: { row: Extract<ConversationRow, { kind: 'day' | 'new' }> }) {
  const { t } = useTranslation();
  const locale = useAppearance((state) => state.locale);
  const timeZone = useAppearance((state) => state.timeZone);
  if (row.kind === 'new') {
    return (
      <li id="chat-new-line" className="flex items-center gap-3 px-4 py-2">
        <span aria-hidden className="h-px flex-1 bg-danger" />
        <span className="text-xs font-semibold text-danger-text">{t('chat.newLabel')}</span>
        <span aria-hidden className="h-px flex-1 bg-danger" />
      </li>
    );
  }
  const label = dayLabel(row.at, locale, timeZone);
  return (
    <li className="mt-(--message-gap) flex items-center gap-3 px-4 py-2">
      <span aria-hidden className="h-px flex-1 bg-border" />
      <time dateTime={row.at} className="text-xs font-medium text-text-muted first-letter:uppercase">
        {label}
      </time>
      <span aria-hidden className="h-px flex-1 bg-border" />
    </li>
  );
}

export function MessageList({ entry, readTo, watching, target, onReached, onRead, empty, onRestore }: MessageListProps) {
  const { t } = useTranslation();
  const { chat, me, people } = useChatContext();
  const timeZone = useAppearance((state) => state.timeZone);
  const scroller = useRef<HTMLDivElement>(null);
  const list = useRef<HTMLOListElement>(null);
  const [drawn, setDrawn] = useState(DRAWN_FIRST);
  const [stuck, setStuck] = useState(true);
  const stuckNow = useRef(true);
  const [fresh, setFresh] = useState(0);
  const [olderError, setOlderError] = useState(false);
  const loading = useRef<'older' | 'newer' | null>(null);
  const placed = useRef(false);
  const anchor = useRef<{ height: number; top: number } | null>(null);
  const newest = useRef(0);

  // A message that was asked for is drawn whatever else is: enough of the end is drawn to reach it.
  const asked = target === null ? -1 : entry.order.indexOf(target);
  const drawnNow = asked < 0 ? drawn : Math.max(drawn, entry.order.length - asked + DRAWN_MORE);
  const slice = useMemo(() => entry.order.slice(Math.max(0, entry.order.length - drawnNow)), [entry.order, drawnNow]);
  const messages = useMemo(() => slice.map((id) => entry.messages.get(id)).filter((message) => message !== undefined), [slice, entry.messages]);
  const rows = useMemo(
    () =>
      conversationRows(messages, {
        readTo,
        me,
        // The day is told in the reader's time zone; a day is the same string for every moment in it.
        dayOf: (moment) => new Intl.DateTimeFormat('en-CA', { timeZone, year: 'numeric', month: '2-digit', day: '2-digit' }).format(new Date(moment)),
      }),
    [messages, readTo, me, timeZone],
  );
  // What this page sent and the server has not answered yet stands at the end, unless the same message is held already.
  const outgoing = useMemo(
    () =>
      entry.outbox.filter(
        (sent) =>
          !messages.some(
            (held) =>
              isMine(held, me) && !held.deleted && Date.parse(held.created_at) >= sent.at - 60_000 && JSON.stringify(held.parts) === JSON.stringify(sent.parts),
          ),
      ),
    [entry.outbox, messages, me],
  );
  const newestSeq = newestHeld(entry);
  // A group that has only its first event line has been written in by nobody yet, and says so under it.
  const talked = rows.some((row) => row.kind === 'message') || outgoing.length > 0;
  const hasContent = talked || rows.some((row) => row.kind === 'system');

  const capture = useCallback(() => {
    const element = scroller.current;
    if (element !== null) anchor.current = { height: element.scrollHeight, top: element.scrollTop };
  }, []);

  const toEnd = useCallback(() => {
    const element = scroller.current;
    if (element !== null) element.scrollTop = element.scrollHeight;
  }, []);

  // What is laid out late (a font, an avatar, a block that is drawn after its text) must not leave the end out of sight:
  // while the reader is at the end, the end is kept in view as the list grows.
  useEffect(() => {
    stuckNow.current = stuck;
  }, [stuck]);
  useEffect(() => {
    const content = list.current;
    const frame = scroller.current;
    if (content === null || frame === null || typeof ResizeObserver === 'undefined') return;
    const watch = new ResizeObserver(() => {
      if (placed.current && stuckNow.current) toEnd();
    });
    // The list itself grows; the room it has shrinks when the box under it does (a reply bar, a second line).
    watch.observe(content);
    watch.observe(frame);
    return () => watch.disconnect();
  }, [toEnd, hasContent]);

  // Where the list opens: at the line of the new messages, else at the end.
  useLayoutEffect(() => {
    if (placed.current || entry.loaded === 'none') return;
    placed.current = true;
    const element = scroller.current;
    const line = document.getElementById('chat-new-line');
    if (element !== null && line !== null) {
      element.scrollTop = Math.max(0, line.offsetTop - NEW_LINE_MARGIN_PX);
      setStuck(element.scrollHeight - element.scrollTop - element.clientHeight < FOLLOW_WITHIN_PX);
    } else {
      toEnd();
    }
    newest.current = newestSeq;
  }, [entry.loaded, newestSeq, toEnd]);

  // What was above the reader stays above them when older messages are laid in front.
  const firstId = slice[0];
  useLayoutEffect(() => {
    const element = scroller.current;
    const held = anchor.current;
    if (element === null || held === null) return;
    anchor.current = null;
    element.scrollTop = held.top + (element.scrollHeight - held.height);
  }, [firstId]);

  // New messages: followed while the reader is at the end, counted otherwise.
  useLayoutEffect(() => {
    if (!placed.current || newestSeq <= newest.current) return;
    const arrived = messages.filter((message) => message.kind === 'message' && message.seq_created > newest.current && !isMine(message, me)).length;
    newest.current = newestSeq;
    if (stuck || entry.outbox.length > 0) toEnd();
    else if (arrived > 0) setFresh((count) => count + arrived);
  }, [newestSeq, messages, me, stuck, toEnd, entry.outbox.length]);

  // What the reader sent stands in view at once.
  useLayoutEffect(() => {
    if (entry.outbox.length > 0) toEnd();
  }, [entry.outbox.length, toEnd]);

  const loadOlder = useCallback(() => {
    if (entry.order.length > drawn) {
      capture();
      setDrawn((count) => count + DRAWN_MORE);
      return;
    }
    if (!entry.hasOlder || loading.current !== null) return;
    const first = entry.order[0];
    const seq = first === undefined ? undefined : entry.messages.get(first)?.seq_created;
    if (seq === undefined) return;
    loading.current = 'older';
    capture();
    setOlderError(false);
    fetchMessages(chat.id, { before: seq })
      .then((reply) => {
        chatWriter.older(chat.id, reply);
        setDrawn((count) => count + MESSAGES_PAGE);
      })
      .catch(() => setOlderError(true))
      .finally(() => {
        loading.current = null;
      });
  }, [chat.id, drawn, entry.hasOlder, entry.messages, entry.order, capture]);

  const loadNewer = useCallback(() => {
    if (entry.loaded !== 'window' || !entry.hasNewer || loading.current !== null) return;
    loading.current = 'newer';
    fetchMessages(chat.id, { after: newestHeld(entry) })
      .then((reply) => chatWriter.newer(chat.id, reply))
      .catch(() => undefined)
      .finally(() => {
        loading.current = null;
      });
  }, [chat.id, entry]);

  const onScroll = () => {
    const element = scroller.current;
    if (element === null || !placed.current) return;
    const away = element.scrollHeight - element.scrollTop - element.clientHeight;
    const atEnd = away < FOLLOW_WITHIN_PX;
    setStuck(atEnd);
    if (atEnd) {
      setFresh(0);
      loadNewer();
    }
    if (element.scrollTop < LOAD_OLDER_WITHIN_PX) loadOlder();
  };

  // A list too short to scroll has nothing to scroll to, and older messages may still be wanted to fill it.
  useEffect(() => {
    const element = scroller.current;
    if (element !== null && placed.current && element.scrollHeight <= element.clientHeight && entry.hasOlder) loadOlder();
  }, [entry.hasOlder, loadOlder, rows.length]);

  // A message asked for: drawn if it is held and not yet drawn, then brought into view.
  useEffect(() => {
    if (target === null || !placed.current) return;
    const element = document.getElementById(`message-${target}`);
    if (element === null) return;
    element.scrollIntoView({ block: 'center' });
    onReached(target);
  }, [target, onReached, rows.length]);

  // The newest message counts as read after two seconds on screen with the tab visible.
  useEffect(() => {
    if (!stuck || !watching || entry.loaded === 'none' || entry.hasNewer || newestSeq === 0) return;
    const timer = setTimeout(() => onRead(newestSeq), READ_AFTER_MS);
    return () => clearTimeout(timer);
  }, [stuck, watching, entry.loaded, entry.hasNewer, newestSeq, onRead]);

  const behind = !stuck || (entry.loaded === 'window' && entry.hasNewer);
  const jump = () => {
    if (entry.loaded === 'window' && entry.hasNewer) {
      placed.current = false;
      reloadChat(chat.id);
    } else {
      toEnd();
    }
    setFresh(0);
    setStuck(true);
  };

  return (
    <div data-chat-history className="relative flex min-h-0 flex-1 flex-col">
      <div ref={scroller} onScroll={onScroll} className="min-h-0 flex-1 overflow-y-auto py-2 [overflow-anchor:none]">
        {entry.hasOlder && (
          <div className="flex justify-center py-2 text-xs text-text-muted" role="status">
            {olderError ? (
              <Button size="sm" variant="ghost" onClick={loadOlder}>
                {t('chat.olderFailed')}
              </Button>
            ) : (
              <span className="inline-flex items-center gap-2">
                <Loader size={16} aria-hidden className="animate-spin" />
                {t('chat.olderLoading')}
              </span>
            )}
          </div>
        )}
        {hasContent ? (
          <ol ref={list} aria-label={t('chat.messageList')} aria-live={stuck ? 'polite' : 'off'} aria-relevant="additions" className="flex flex-col">
            {rows.map((row) =>
              row.kind === 'message' || row.kind === 'system' ? (
                <Message key={row.key} message={row.message} head={row.kind === 'message' && row.head} />
              ) : (
                <Divider key={row.key} row={row} />
              ),
            )}
            {outgoing.map((sent) => (
              <OutgoingMessage key={sent.clientId} chatId={chat.id} sent={sent} author={me === null ? null : (people.get(me) ?? null)} onRestore={onRestore === undefined ? undefined : () => onRestore(sent)} />
            ))}
          </ol>
        ) : (
          empty
        )}
        {hasContent && !talked && <div className="py-6">{empty}</div>}
      </div>
      {behind && hasContent && (
        <div className="pointer-events-none absolute inset-x-0 bottom-3 flex justify-center">
          <Button
            size="sm"
            variant="secondary"
            icon={<ArrowDown aria-hidden />}
            className={cx('pointer-events-auto shadow-md')}
            onClick={jump}
          >
            {fresh > 0 ? t('chat.jumpNew', { count: fresh }) : t('chat.jumpLatest')}
          </Button>
        </div>
      )}
    </div>
  );
}
