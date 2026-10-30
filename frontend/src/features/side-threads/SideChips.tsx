/** The open side threads of a thread, as one chip in its header, with what each is and a way to close it.
 *
 * While a side thread is open the thread says so where its facts are: how many are open, of how many it may have. The
 * chip opens the list: each with its name, whether it is answering, its inline lane or panel, and
 * Close for whoever made it or manages the thread. The list is kept current by what the thread itself says of its side
 * threads, which changes the moment one is started or closed, on any device.
 */
import { useQuery } from '@tanstack/react-query';
import { MessageSquareDashed } from 'lucide-react';
import { useTranslation } from 'react-i18next';
import { useState } from 'react';
import { useStore } from 'zustand';

import { ApiError, client } from '@/api/client';
import { sidesQuery } from '@/api/forks';
import { useCloseSide } from '@/features/side-threads/ask';
import { sideLaneKey, timelineUi } from '@/features/timeline/state';
import { revealView } from '@/features/workbench/openView';
import type { NodeRef, Thread } from '@/generated/api-types';
import { holds } from '@/lib/permissions';
import { useSocketStatus } from '@/realtime/live';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { useThreadPart } from '@/realtime/stores/thread';
import { Button } from '@/ui/Button';
import { ErrorState } from '@/ui/ErrorState';
import { Popover } from '@/ui/Popover';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { Tooltip } from '@/ui/Tooltip';

interface ListedSide { ref: NodeRef; maker: string; source?: string; display?: Thread['display'] }

function SideEntry({ entry, parent, onRevealItem, onOpened }: { entry: ListedSide; parent: Thread; onRevealItem?: (id: string) => void; onOpened?: () => void }) {
  const { t } = useTranslation();
  const read = useThreadReadRef(parent.id);
  const me = useStore(client.session, (state) => state.userId);
  const live = useSocketStatus().phase === 'ready';
  const held = useThreadPart(entry.ref.id, (view) => view.thread);
  const ended = useThreadPart(entry.ref.id, (view) => view.closed);
  const close = useCloseSide(parent.id);
  const name = held?.name ?? entry.ref.name ?? t('side.lane.unnamed');
  const display = held?.display ?? entry.display ?? entry.ref.thread_display;
  const mayClose = me === entry.maker || holds(parent.access, 'thread.manage');
  const openPanel = () => {
    timelineUi.setStep(read, sideLaneKey(entry.ref.id), false);
    revealView('side-thread', { thread: parent.id, side: entry.ref.id });
  };
  if (ended === 'deleted' || entry.ref.redacted) return null;
  return <li className="flex min-w-0 flex-wrap items-center gap-1 text-sm">
    <ThreadStateIcon display={display} />
    <Tooltip content={name}><button type="button" className="focus-inset min-h-8 min-w-0 flex-1 truncate rounded-md px-2 text-left hover:bg-bg-muted" onClick={() => {
      if (entry.source === undefined) openPanel();
      else { timelineUi.openSide(read, entry.ref.id); onRevealItem?.(entry.source); }
      onOpened?.();
    }}>{name}</button></Tooltip>
    <Button size="sm" variant="ghost" onClick={() => { openPanel(); onOpened?.(); }}>{t('side.lane.openPanel')}</Button>
    {mayClose && <Button size="sm" variant="ghost" disabled={!live} pending={close.isPending} onClick={() => close.mutate(entry.ref.id)}>{t('side.close')}</Button>}
    {close.error !== null && <ErrorState compact error={close.error} />}
  </li>;
}

/** The same live side-thread list in its chip popover or the session menu's dialog. */
export function SideThreadList({ thread, onRevealItem, onOpened }: { thread: Thread; onRevealItem?: (id: string) => void; onOpened?: () => void }) {
  const { t } = useTranslation();
  const open = thread.sides?.open ?? 0;
  const sides = useQuery({ ...sidesQuery(thread.id, 'open'), enabled: open > 0, staleTime: Infinity, refetchOnWindowFocus: false, refetchOnReconnect: false, retry: false });
  const view = useThreadPart(thread.id, (held) => held);
  const listed = new Map<string, ListedSide>((sides.data ?? []).map((side) => [side.id, { ref: { id: side.id, name: side.name, kind: 'thread', redacted: false }, maker: side.owner.id, display: side.display }]));
  // The parent feed already reports openings and closings; reading the list never starts a poll
  // or acquires every side's feed. A missing source in this window opens directly in the panel.
  for (const id of view.order) {
    const item = view.items.get(id);
    if (item?.kind !== 'marker') continue;
    if (item.payload.marker === 'side_started') listed.set(item.payload.data.side.id, { ...listed.get(item.payload.data.side.id), ref: item.payload.data.side, maker: item.payload.data.by.id, source: id });
    else if (item.payload.marker === 'side_closed') listed.delete(item.payload.data.side.id);
  }
  if (thread.sides === null) return null;
  if (sides.error instanceof ApiError && (sides.error.status === 403 || sides.error.status === 404)) return <ErrorState compact error={sides.error} />;
  return (
      <div className="flex flex-col gap-2">
        <p className="text-sm text-text-muted">{t('side.chip.limit', { open, max: thread.sides.max })}</p>
        <ul aria-label={t('side.chip.label')} className="flex flex-col gap-1">
          {[...listed.values()].map((entry) => <SideEntry key={entry.ref.id} entry={entry} parent={thread} {...(onRevealItem === undefined ? {} : { onRevealItem })} {...(onOpened === undefined ? {} : { onOpened })} />)}
        </ul>
        {sides.error !== null && <ErrorState compact error={sides.error} onRetry={() => void sides.refetch()} />}
      </div>
  );
}

export function SideChips({ thread }: { thread: Thread }) {
  const { t } = useTranslation();
  const [shown, setShown] = useState(false);
  const open = thread.sides?.open ?? 0;
  if (open === 0) return null;
  return <Popover open={shown} onOpenChange={setShown} label={t('side.chip.label')} className="w-(--chip-width)" trigger={
    <button type="button" data-side-chip className="inline-flex items-center gap-1 rounded-full border border-border px-2 text-sm text-text-muted hover:bg-bg-muted hover:text-text"><MessageSquareDashed aria-hidden />{t('side.chip.text', { count: open })}</button>
  }><SideThreadList thread={thread} onOpened={() => setShown(false)} /></Popover>;
}
