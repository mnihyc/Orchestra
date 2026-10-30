/** Copy, link, pin, quote, discuss, forward, select, what the agent received, and fork or ask on the side from any message.
 *
 * What can be done with an item, in the same list whether it is reached by the button at its end or
 * by the right mouse button: copy what it shows, copy its link, bring it to the thread's chat as a
 * quote, pick it (with others, to forward, quote or discuss; a long press on a touch screen picks it
 * too), pin it for the others. Each is left out
 * where the reader may not do it (pinning is for whoever drives the thread). While items are being
 * picked each has a box to tick in place of its menu, and an item that messages of the thread's chat
 * quote carries their number. What a person said opens, beside it, what the agent received with it: the
 * view of what was given, narrowed to its turn.
 */
import { useMutation } from '@tanstack/react-query';
import { Copy, Ellipsis, GitFork, Link as LinkIcon, ListChecks, MessageSquareDashed, MessageSquareQuote, Pin, PinOff, RotateCcw, ScrollText } from 'lucide-react';
import { type ReactNode, useContext, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import { pinItem } from '@/api/threads';
import { openChange } from '@/features/change/target';
import { discussInChat } from '@/features/chat/discuss';
import { openGiven } from '@/features/inspector/givenFocus';
import { openSideAsk } from '@/features/side-threads/ask';
import { sideReadingAvailable, sideRefusal } from '@/features/side-threads/availability';
import type { Item, Turn } from '@/generated/api-types';
import { RestoreDialog } from '@/features/source/Restore';
import { ChatRefs } from '@/features/timeline/ChatRefs';
import { useTimelineContext } from '@/features/timeline/context';
import { ViewRouteContext } from '@/features/workbench/viewOpening';
import { isForwarded } from '@/lib/content';
import { threadViewOf, threadWriter, useThreadPart } from '@/realtime/stores/thread';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { defaultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { copyText } from '@/ui/clipboard';
import { toast } from '@/ui/Toast';

/** What an item says, as text a person would copy. */
export function itemText(item: Item): string {
  switch (item.kind) {
    case 'user_message':
      return item.payload.parts.map((part) => (isForwarded(part) ? part.text : part.type === 'text' ? part.text : part.name)).join('\n');
    case 'agent_message':
      return item.payload.text;
    case 'reasoning':
      return item.payload.summary;
    case 'plan':
      return item.payload.steps.map((step) => `- ${step.text}`).join('\n');
    case 'tool_call': {
      const tool = item.payload.tool;
      if (tool.kind === 'command') return `$ ${tool.command}\n${tool.output.inline}`;
      if (tool.kind === 'file_change') return tool.changes.map((change) => change.diff ?? change.path).join('\n');
      return item.payload.title;
    }
    case 'announcement':
      return `${item.payload.title}\n${item.payload.body}`;
    case 'error':
      return item.payload.message;
    case 'approval':
    case 'marker':
      return '';
  }
}

/** The address of an item: the thread's, with the item named. */
export function itemLink(target: ThreadReadTarget, itemId: string): string {
  const read = typeof target === 'string' ? defaultThreadRead(target) : target;
  const url = new URL(`/thread/${read.threadId}`, location.origin);
  url.searchParams.set('item', itemId);
  const continuation = threadViewOf(read).read?.projection?.viewed_continuation_id;
  if (continuation !== undefined) url.searchParams.set('continuation', continuation);
  return url.toString();
}

/** The address of several items: the thread's, with the first brought into view and all of them lit. */
export function itemsLink(target: ThreadReadTarget, itemIds: readonly string[]): string {
  const read = typeof target === 'string' ? defaultThreadRead(target) : target;
  const url = new URL(`/thread/${read.threadId}`, location.origin);
  const [first] = itemIds;
  if (first !== undefined) url.searchParams.set('item', first);
  if (itemIds.length > 1) url.searchParams.set('items', itemIds.join(','));
  const continuation = threadViewOf(read).read?.projection?.viewed_continuation_id;
  if (continuation !== undefined) url.searchParams.set('continuation', continuation);
  return url.toString();
}

/** How long a finger rests on an item before the conversation is put in selection mode with it picked: less than the
 * menu that a long touch opens on the item, so that the press picks the item instead. */
const LONG_PRESS_MS = 450;

/** How far, in pixels, a resting finger may drift before the press is taken for a scroll and let go. */
const LONG_PRESS_DRIFT = 10;

/** The entries for an item, and `extra` ones of its kind first. */
export function useItemEntries(item: Item, extra: readonly MenuEntry[] = [], onRestore?: (turn: Turn) => void): MenuEntry[] {
  const route = useContext(ViewRouteContext);
  const { t } = useTranslation();
  const context = useTimelineContext();
  const read = useThreadReadRef(context.threadId);
  const pin = useMutation({
    mutationFn: (pinned: boolean) => pinItem(read, item.id, pinned),
    onSuccess: (row) => threadWriter.apply(read, { items: [row] }),
    onError: () => toast({ tone: 'danger', title: t('timeline.actions.notPinned') }),
  });
  const copy = (text: string) => void copyText(text);
  const text = itemText(item);
  // From a message a fork or a side thread takes the history up to it: through the turn it ends, or for what a person said, through the turn before it.
  const turn = useThreadPart(context.threadId, (view) => (item.turn_id === null ? undefined : view.turns.get(item.turn_id)));
  const thread = useThreadPart(context.threadId, (view) => view.thread);
  const isSide = thread !== null && thread.side_of !== null;
  const sideAllowed = thread?.sides?.allowed !== false;
  const asideRefusal = sideRefusal(thread, t);
  // Inspection offers no conversation act, including selection, discussion, pinning or forwarding.
  if (context.inspection !== undefined) return [];
  const entries: MenuEntry[] = [...extra];
  if (entries.length > 0) entries.push({ type: 'separator', id: 'extra-end' });
  if (text !== '') entries.push({ id: 'copy', label: t('timeline.actions.copy'), icon: <Copy aria-hidden />, onSelect: () => copy(text) });
  entries.push({ id: 'link', label: t('timeline.actions.copyLink'), icon: <LinkIcon aria-hidden />, onSelect: () => copy(itemLink(read, item.id)) });
  entries.push({
    id: 'discuss',
    label: t('timeline.actions.discuss'),
    icon: <MessageSquareQuote aria-hidden />,
    // The item stands in the composer of the thread's chat as a quote, and the chat comes to the front; nothing is sent until the person sends it.
    onSelect: () => {
      if (!discussInChat(context.threadId, [item.id], route)) toast({ tone: 'danger', title: t('timeline.actions.noChat') });
    },
  });
  entries.push({ id: 'select', label: t('timeline.actions.select'), icon: <ListChecks aria-hidden />, onSelect: () => context.selection.start(item.id) });
  if (turn !== undefined && item.kind === 'agent_message') {
    entries.push({ id: 'select-turn', label: t('timeline.selection.turn'), icon: <ListChecks aria-hidden />, onSelect: () => {
      const view = threadViewOf(read);
      context.selection.pick(view.order.filter((id) => view.items.get(id)?.turn_id === turn.id));
    } });
    if (onRestore !== undefined && turn.completed_at !== null && turn.before_snapshot_id !== null && thread?.environment !== null
      && thread?.environment_permissions.includes('files.view') === true && thread.environment_permissions.includes('files.manage')) {
      entries.push({ id: 'restore-turn', label: t('source.restore.open'), icon: <RotateCcw aria-hidden />, onSelect: () => onRestore(turn) });
    }
  }
  if (typeof item.model_input_id === 'string') {
    const record = item.model_input_id;
    entries.push({ id: 'received', label: t('threadViews.given.received'), icon: <ScrollText aria-hidden />, onSelect: () => { openGiven(context.threadId, { record }); } });
  } else if (turn !== undefined && item.kind === 'user_message') {
    // The words the agent received with what the person said, beside it: the view of what was given, narrowed to its turn.
    entries.push({
      id: 'received',
      label: t('threadViews.given.received'),
      icon: <ScrollText aria-hidden />,
      onSelect: () => {
        openGiven(context.threadId, { turn: turn.id });
      },
    });
  }
  if (turn !== undefined && (item.kind === 'user_message' || item.kind === 'agent_message')) {
    const through = item.kind === 'user_message' ? turn.ordinal - 1 : turn.ordinal;
    const target = { kind: 'thread' as const, id: context.threadId, name: context.threadName };
    entries.push({
      id: 'fork-here',
      label: t('change.item.fork'),
      icon: <GitFork aria-hidden />,
      onSelect: () => openChange(target, { kind: 'fork', toTurn: through, ...(item.kind === 'user_message' && text !== '' ? { say: text } : {}) }),
    });
    if (context.canInteract && !isSide && sideAllowed && sideReadingAvailable(thread)) {
      entries.push({ id: 'side-here', label: t('side.item.ask'), icon: <MessageSquareDashed aria-hidden />, disabled: asideRefusal, onSelect: () => {
        if (asideRefusal === false) openSideAsk({ threadId: context.threadId, threadName: context.threadName, toTurn: through, text: '', read });
      } });
    }
  }
  if (context.canInteract) {
    const pinned = item.pinned !== null;
    entries.push({
      id: 'pin',
      label: pinned ? t('timeline.actions.unpin') : t('timeline.actions.pin'),
      icon: pinned ? <PinOff aria-hidden /> : <Pin aria-hidden />,
      onSelect: () => pin.mutate(!pinned),
    });
  }
  return context.readOnly === true ? entries.map((entry) => 'onSelect' in entry
    && ['discuss', 'select', 'select-turn', 'fork-here', 'side-here'].includes(entry.id)
    ? { ...entry, disabled: t('messageVersions.activeActions') } : entry) : entries;
}

export interface ItemFrameProps {
  item: Item;
  /** Entries of the item's own kind, put before the common ones. */
  extra?: readonly MenuEntry[];
  /** What the item is, for the name of its menu. */
  label: string;
  children: ReactNode;
  className?: string;
}

/** Wraps an item so that its menu opens under the right mouse button and from a button at its end. */
export function ItemFrame({ item, extra, label, children, className }: ItemFrameProps) {
  const { t } = useTranslation();
  const [restoring, setRestoring] = useState<Turn | null>(null);
  const entries = useItemEntries(item, extra, setRestoring);
  const context = useTimelineContext();
  const thread = useThreadPart(context.threadId, (view) => view.thread);
  const { selection } = context;
  const body = useRef<HTMLDivElement>(null);
  const acts = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const content = body.current;
    const menu = acts.current;
    if (content === null || menu === null) return;
    // Measure the first painted text line, not an assumed padding or the height of the whole item.
    // Messages, tool rows and multiline cards use different first-line insets and font metrics.
    const align = () => {
      const root = menu.parentElement;
      if (root === null) return;
      const walker = document.createTreeWalker(content, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node !== null; node = walker.nextNode()) {
        if (node.textContent?.trim() === '' || node.parentElement?.closest('[aria-hidden="true"], [role="img"], svg, .avatar, [hidden], .sr-only') !== null) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        if (typeof range.getClientRects !== 'function') return;
        const line = range.getClientRects()[0];
        if (line === undefined || line.width === 0 || line.height <= 1) continue;
        menu.style.top = `${line.top + line.height / 2 - root.getBoundingClientRect().top - menu.getBoundingClientRect().height / 2}px`;
        break;
      }
    };
    align();
    const hasFonts = Reflect.has(document, 'fonts');
    if (hasFonts) document.fonts.addEventListener('loadingdone', align);
    if (typeof ResizeObserver === 'undefined') return () => { if (hasFonts) document.fonts.removeEventListener('loadingdone', align); };
    const watch = new ResizeObserver(align);
    watch.observe(content);
    watch.observe(menu);
    return () => { watch.disconnect(); if (hasFonts) document.fonts.removeEventListener('loadingdone', align); };
  }, [item.id, context.inspection, selection.active]);
  // A long press on a touch screen begins picking with this item; moving the finger, lifting it early or leaving lets go.
  const press = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pressedAt = useRef<{ x: number; y: number } | null>(null);
  const letGo = () => {
    if (press.current !== null) {
      clearTimeout(press.current);
      press.current = null;
    }
  };
  // A press still held when the item goes away is let go with it.
  useEffect(() => {
    const held = press;
    return () => {
      if (held.current !== null) clearTimeout(held.current);
    };
  }, []);
  if (context.inspection !== undefined) return <div data-item={item.id} className={className}>{children}</div>;
  if (selection.active) {
    // Items are being picked: a box in place of the menu, and the item itself can be pressed to pick it.
    const picked = selection.has(item.id);
    return (
      <div
        data-item={item.id}
        data-picked={picked ? '' : undefined}
        onClick={(event) => {
          if (!(event.target as HTMLElement).closest('a,button,input')) selection.toggle(item.id, event.shiftKey);
        }}
        className={cx('relative flex cursor-pointer items-start gap-2 rounded-lg', className, picked && 'ring-2 ring-accent')}
      >
        <input
          type="checkbox"
          checked={picked}
          aria-label={t('timeline.selection.pick')}
          onChange={() => undefined}
          onClick={(event) => {
            event.stopPropagation();
            selection.toggle(item.id, event.shiftKey);
          }}
          className="mt-1 size-4 shrink-0 accent-accent-solid"
        />
        <div className="min-w-0 flex-1">{children}</div>
      </div>
    );
  }
  return (
    <><ContextMenu entries={entries} label={label}>
      <div
        data-item={item.id}
        className={cx('group/item relative', className, context.lit?.has(item.id) === true && 'rounded-lg bg-accent-subtle ring-2 ring-accent')}
        onPointerDown={(event) => {
          if (context.readOnly === true || event.pointerType !== 'touch') return;
          letGo();
          pressedAt.current = { x: event.clientX, y: event.clientY };
          press.current = setTimeout(() => {
            press.current = null;
            selection.start(item.id);
          }, LONG_PRESS_MS);
        }}
        onPointerUp={letGo}
        onPointerCancel={letGo}
        onPointerLeave={letGo}
        onPointerMove={(event) => {
          const from = pressedAt.current;
          if (from !== null && Math.hypot(event.clientX - from.x, event.clientY - from.y) > LONG_PRESS_DRIFT) letGo();
        }}
      >
        <div ref={body} data-item-body>{children}</div>
        {item.chat_refs > 0 && <ChatRefs count={item.chat_refs} />}
        <div ref={acts} data-item-acts className="absolute top-0 right-0 opacity-0 transition-opacity group-focus-within/item:opacity-100 group-hover/item:opacity-100">
          <Menu label={label} align="end" entries={entries} trigger={<IconButton size="sm" label={t('timeline.actions.more')} icon={<Ellipsis aria-hidden />} />} />
        </div>
      </div>
    </ContextMenu>
    {restoring?.before_snapshot_id !== null && restoring !== null && thread !== null && <RestoreDialog thread={thread} snapshotId={restoring.before_snapshot_id} onClose={() => setRestoring(null)} />}</>
  );
}
