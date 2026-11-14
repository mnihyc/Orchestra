/** Where the reader stands among the people's messages, and the moves between them: previous and next by the keys and the buttons, and the sticky prompt.
 *
 * The list reports the item at the top of the view and the items in view. The message the reader is in the middle of is
 * the last one said at or before the top; once its row has scrolled above the view it is the sticky prompt. Previous and next
 * go to the one before or after the top of the view (or, until the list has said where a move went, before or after that).
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { type PersonMessage, messageAt, messageBeside, personMessages } from '@/features/thread/messages';
import { bindKey } from '@/shell/keys';
import { useThreadPart } from '@/realtime/stores/thread';

export interface MessageNav {
  messages: readonly PersonMessage[];
  /** The message that began the turn in view, while it has scrolled above the view. */
  pinned: PersonMessage | null;
  /** Its place among the messages, from 1, where every message of the thread is held. */
  place: number | null;
  /** Where every message of the thread is held, the count is the thread's own. */
  whole: boolean;
  canPrevious: boolean;
  canNext: boolean;
  previous: () => boolean;
  next: () => boolean;
  go: (id: string) => void;
}

export function useMessageNav(threadId: string, where: { top: string | null; visible: ReadonlySet<string> }, reveal: (id: string) => void, toStart: () => void): MessageNav {
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useThreadPart(threadId, (view) => view.order);
  const hasOlder = useThreadPart(threadId, (view) => view.hasOlder);
  const messages = useMemo(() => personMessages(items, order), [items, order]);
  // The message just moved to, and the top of the view when it was: until the list says that the top is another, previous and
  // next count from that message, so that keys that repeat do not wait for the list to say where the view went.
  const [moved, setMoved] = useState<{ id: string; top: string | null } | null>(null);
  const topSeq = where.top === null ? null : items.get(where.top)?.seq_created ?? null;
  const movedSeq = moved !== null && moved.top === where.top ? items.get(moved.id)?.seq_created : undefined;
  const base = movedSeq ?? topSeq;
  const middle = topSeq === null ? null : messageAt(messages, topSeq);
  const pinned = middle !== null && !where.visible.has(middle.id) && where.top !== middle.id ? middle : null;
  const go = (id: string) => {
    setMoved({ id, top: where.top });
    reveal(id);
  };
  const beside = (direction: -1 | 1): PersonMessage | null => {
    if (base === null) return direction === -1 ? messages.at(-1) ?? null : null;
    return messageBeside(messages, base, direction);
  };
  const previous = (): boolean => {
    const message = beside(-1);
    if (message !== null) go(message.id);
    else if (hasOlder) toStart();
    else return false;
    return true;
  };
  const next = (): boolean => {
    const message = beside(1);
    if (message === null) return false;
    go(message.id);
    return true;
  };
  return {
    messages,
    pinned,
    place: !hasOlder && pinned !== null ? messages.indexOf(pinned) + 1 : null,
    whole: !hasOlder,
    canPrevious: messages.length > 0 && (base === null || messageBeside(messages, base, -1) !== null || hasOlder),
    canNext: base !== null && messageBeside(messages, base, 1) !== null,
    previous,
    next,
    go,
  };
}

/** Ctrl+Alt+Up and Ctrl+Alt+Down (⌥⌘↑ and ⌥⌘↓ on a Mac) move between the people's messages, while this thread is the one the keyboard is in. */
export function useMessageKeys(nav: MessageNav, enabled: boolean): void {
  const latest = useRef(nav);
  useEffect(() => { latest.current = nav; });
  const previous = useCallback(() => latest.current.previous(), []);
  const next = useCallback(() => latest.current.next(), []);
  useEffect(() => {
    if (!enabled) return;
    const undo = [
      bindKey('thread-previous-message', 'Mod+Alt+ArrowUp', previous, 'thread.messages.keys'),
      bindKey('thread-next-message', 'Mod+Alt+ArrowDown', next, 'thread.messages.keys'),
    ];
    return () => { for (const remove of undo) remove(); };
  }, [enabled, previous, next]);
}
