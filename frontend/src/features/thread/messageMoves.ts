/** Where the reader stands among the people's messages, and the moves between them: previous and next by the keys and the buttons, and the sticky prompt.
 *
 * The list reports the item at the top of the view and the items in view. The message the reader is in the middle of is
 * the last one said at or before the top; once its row has scrolled above the view it is the sticky prompt. Previous and next
 * go to the one before or after the top of the view (or, until the list has said where a move went, before or after that).
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { fetchMessageNeighbors } from '@/api/threads';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { type PersonMessage, messageAt, messageBeside, personMessages } from '@/features/thread/messages';
import { bindKey } from '@/shell/keys';
import { threadViewOf, useThreadPart } from '@/realtime/stores/thread';

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
  moving: boolean;
  error: unknown;
  retry: () => void;
}

interface NeighborDemand {
  controller: AbortController;
  scope: AbortController | null;
  anchor: string;
  seq: number;
  direction: -1 | 1;
  loading: boolean;
  error: unknown;
}

export function useMessageNav(threadId: string, where: { top: string | null; visible: ReadonlySet<string> }, reveal: (id: string) => void): MessageNav {
  const read = useThreadReadRef(threadId);
  const items = useThreadPart(threadId, (view) => view.items);
  const order = useThreadPart(threadId, (view) => view.order);
  const hasOlder = useThreadPart(threadId, (view) => view.hasOlder);
  const hasNewer = useThreadPart(threadId, (view) => view.hasNewer);
  const scope = useThreadPart(threadId, (view) => view.read?.controller ?? null);
  const messages = useMemo(() => personMessages(items, order), [items, order]);
  const [moved, setMoved] = useState<{ id: string; seq: number | null; top: string | null; scope: AbortController | null } | null>(null);
  const [demand, setDemand] = useState<NeighborDemand | null>(null);
  const [edges, setEdges] = useState<{ before?: number; after?: number; scope: AbortController | null } | null>(null);
  const request = useRef<NeighborDemand | null>(null);
  useEffect(() => () => { if (request.current?.scope === scope) request.current.controller.abort(); }, [read, scope]);
  const topSeq = where.top === null ? null : items.get(where.top)?.seq_created ?? null;
  const atMove = moved?.scope === scope && moved.top === where.top ? moved : null;
  const base = atMove === null ? topSeq : atMove.seq ?? items.get(atMove.id)?.seq_created ?? null;
  const anchor = atMove?.id ?? where.top;
  const middle = topSeq === null ? null : messageAt(messages, topSeq);
  const pinned = middle !== null && !where.visible.has(middle.id) && where.top !== middle.id ? middle : null;
  const current = demand?.scope === scope && !demand.controller.signal.aborted ? demand : null;
  const moving = current?.loading === true;
  const go = (id: string, seq = items.get(id)?.seq_created ?? null) => {
    request.current?.controller.abort(); setDemand(null);
    setMoved({ id, seq, top: where.top, scope });
    reveal(id);
  };
  const neighbors = (target: string, seq: number, direction: -1 | 1) => {
    request.current?.controller.abort();
    const next: NeighborDemand = { controller: new AbortController(), scope, anchor: target, seq, direction, loading: true, error: null };
    request.current = next; setDemand(next);
    const currentRead = () => request.current === next && !next.controller.signal.aborted && (threadViewOf(read).read?.controller ?? null) === scope;
    void fetchMessageNeighbors(read, target, next.controller.signal).then((page) => {
      if (!currentRead()) return;
      const points = page.entries.filter((entry) => entry.kind === 'user_message');
      const entry = direction === -1 ? points.findLast((entry) => entry.seq < seq) : points.find((entry) => entry.seq > seq);
      if (entry !== undefined) go(entry.item_id, entry.seq);
      else {
        setEdges((held) => ({ ...(held?.scope === scope ? held : {}), scope, [direction === -1 ? 'before' : 'after']: seq }));
        setDemand({ ...next, loading: false });
      }
    }).catch((error: unknown) => { if (currentRead()) setDemand({ ...next, loading: false, error }); });
  };
  const inside = base !== null && items.has(atMove?.id ?? where.top ?? '');
  const beside = (direction: -1 | 1): PersonMessage | null => {
    if (base === null) return atMove === null && direction === -1 ? messages.at(-1) ?? null : null;
    return inside ? messageBeside(messages, base, direction) : null;
  };
  const allowed = (direction: -1 | 1) => {
    if (moving || atMove !== null && base === null) return false;
    if (beside(direction) !== null) return true;
    if (base === null || anchor === null) return false;
    const exhausted = edges?.scope === scope && (direction === -1 ? edges.before : edges.after) === base;
    return !exhausted && (!inside || (direction === -1 ? hasOlder : hasNewer));
  };
  const move = (direction: -1 | 1): boolean => {
    if (moving) return true;
    if (!allowed(direction)) return false;
    const message = beside(direction);
    if (message !== null) go(message.id, message.seq);
    else if (anchor !== null && base !== null) neighbors(anchor, base, direction);
    return true;
  };
  const whole = !hasOlder && !hasNewer;
  return {
    messages, pinned, whole,
    place: whole && pinned !== null ? messages.indexOf(pinned) + 1 : null,
    canPrevious: allowed(-1), canNext: allowed(1),
    previous: () => move(-1), next: () => move(1), go, moving,
    error: current?.error ?? null,
    retry: () => { if (current !== null) neighbors(current.anchor, current.seq, current.direction); },
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
