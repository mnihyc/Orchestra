/** Each full conversation uses the same local Find bar as other independently targeted views. */
import { MessageSquareText } from 'lucide-react';
import { type RefObject, useCallback, useEffect, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { ITEM_KINDS } from '@/api/search';
import { useRegisterCommands, type Command } from '@/features/finder/commands';
import { useViewFind } from '@/features/search/ViewFind';
import { useViewInstance } from '@/features/workbench/instance';
import type { TimelineHandle } from '@/features/timeline/Timeline';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { threadViewOf } from '@/realtime/stores/thread';
import { sameThreadProjection } from '@/realtime/threadScope';
import type { SearchHit } from '@/generated/api-types';

const owners = new Map<string, { thread: string; show: () => void }>();
export function openFind(threadId: string, owner?: string): void {
  const focused = document.activeElement instanceof HTMLElement ? document.activeElement.closest<HTMLElement>('[data-view-instance]')?.dataset.viewInstance : undefined;
  const entry = owners.get(owner ?? focused ?? threadId);
  if (entry?.thread === threadId) entry.show();
}

export interface FindInConversationProps {
  threadId: string;
  timeline: RefObject<TimelineHandle | null>;
  root: RefObject<HTMLDivElement | null>;
}
export function FindInConversation({ threadId, timeline, root }: FindInConversationProps) {
  const { t } = useTranslation();
  const instance = useViewInstance();
  const owner = instance?.id ?? threadId;
  const read = useThreadReadRef(threadId);
  const hit = useCallback((found: SearchHit) => {
    if (found.kind !== 'item' || found.thread.id !== threadId) return;
    const projection = threadViewOf(read).read?.projection;
    if (projection !== undefined && projection !== null && (found.projection === undefined || found.projection === null || !sameThreadProjection(projection, found.projection))) return;
    timeline.current?.scrollToItem(found.item.id);
  }, [read, threadId, timeline]);
  const find = useViewFind({ root, filters: { scope: threadId, kinds: ITEM_KINDS }, historyThread: read,
    label: t('search.find.label'), onHit: hit, content: '[role="log"]' });
  useEffect(() => {
    const entry = { thread: threadId, show: find.show };
    owners.set(owner, entry);
    return () => { if (owners.get(owner) === entry) owners.delete(owner); };
  }, [owner, threadId, find.show]);
  const commands = useMemo<Command[]>(() => [{ id: 'thread.find', kind: 'place', title: (translate) => translate('search.find.command'),
    keywords: (translate) => translate('search.find.words').split(/\s+/), about: (translate) => translate('search.find.commandAbout'),
    icon: MessageSquareText, run: () => openFind(threadId, owner) }], [threadId, owner]);
  useRegisterCommands(commands);
  return find.bar;
}
