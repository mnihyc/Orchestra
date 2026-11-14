/** Contained work states, idle and manual Done; visible drop targets stay in place during a drag while the page can still be scrolled explicitly.
 *
 * A project's topics stand in the shared order of work, followed by quiet work and manual Done.
 * A topic stands in exactly one, the most urgent state among its threads, so only the
 * Done column can be moved into or out of: dragging a card there, or choosing "Mark done" in its menu,
 * sets its status, and dragging it out reopens it. Nothing is moved before the server has answered; the
 * card shows that it is waiting meanwhile.
 */
import { DndContext, type DragEndEvent, DragOverlay, KeyboardSensor, PointerSensor, useDraggable, useDroppable, useSensor, useSensors } from '@dnd-kit/core';
import { useMutation } from '@tanstack/react-query';
import { defaultRangeExtractor, useVirtualizer } from '@tanstack/react-virtual';
import { CircleCheck, MessageSquareText, type LucideIcon } from 'lucide-react';
import { StateIcon } from '@/ui/StateChip';
import { THREAD_URGENCY, isThreadUrgency } from '@/ui/ThreadState';
import { NodeIcon } from '@/ui/NodeIcon';
import { type CSSProperties, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { EmptyState } from '@/ui/EmptyState';

import { AttentionChips } from '@/features/org/ProjectCards';
import { useIsPending } from '@/features/navigation/NodeMenu';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { PlaceLink } from '@/features/navigation/places';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { lastActivityOf, TopicTags, topicAttention } from '@/features/project/TopicList';
import { TopicPath } from '@/features/project/TopicPath';
import { useStatusChange } from '@/features/topic/statusUndo';
import { TopicMenu } from '@/features/topic/TopicMenu';
import type { Topic, TopicStatus } from '@/generated/api-types';
import { cx } from '@/ui/cx';
import { holds } from '@/lib/permissions';
import { formatNumber } from '@/lib/format';
import { useErrorText } from '@/ui/errorText';
import { RelativeTime } from '@/ui/RelativeTime';
import { ScrollArea } from '@/ui/ScrollArea';
import { toast } from '@/ui/Toast';

export type BoardColumn = (typeof THREAD_URGENCY)[number] | 'idle' | 'done';

export const BOARD_COLUMNS: readonly BoardColumn[] = [...THREAD_URGENCY, 'idle', 'done'];

/** The column a topic stands in: done, or by the most urgent state among its threads. */
export function columnOf(topic: Pick<Topic, 'status' | 'attention' | 'thread_display_state'>): BoardColumn {
  if (topic.status === 'done') return 'done';
  return isThreadUrgency(topic.thread_display_state) ? topic.thread_display_state : 'idle';
}

/** The status a card takes when it is dropped into a column, or `null` where dropping it there is no move at all. */
export function statusOnDrop(from: BoardColumn, to: BoardColumn): TopicStatus | null {
  if (from === to) return null;
  if (to === 'done') return 'done';
  // Out of Done a topic is open again, and which of the other columns it then stands in is for its threads to say.
  return from === 'done' ? 'open' : null;
}

const HEADINGS: Partial<Record<BoardColumn, { Icon: LucideIcon; tone: string }>> = {
  idle: { Icon: MessageSquareText, tone: 'text-text-subtle' },
  done: { Icon: CircleCheck, tone: 'text-success' },
};

const CARD = 'group relative flex flex-col gap-2 rounded-lg border border-border bg-surface p-3 transition-colors';

/** What a card says of a topic. It is drawn in its column and, while it is carried, under the pointer. */
function CardFace({ topic, menu = true, showPaths = false }: { topic: Topic; menu?: boolean; showPaths?: boolean }) {
  return (
    <>
      <div className="flex items-start gap-2"><NodeIcon kind="topic" color={topic.color} own={topic.custom_color} done={topic.status === 'done'} />
        <div className="flex min-w-0 flex-1 flex-col">
          <PlaceLink place={topic} className="truncate font-medium leading-(--control-sm) text-text hover:underline">
            {topic.name}
          </PlaceLink>
          {topic.description !== '' && <span className="line-clamp-2 text-sm wrap-anywhere text-text-muted">{topic.description}</span>}
        </div>
        <AsAdminMark of={adminAccessOf(topic)?.place} />{isThreadUrgency(topic.thread_display_state) && <StateIcon state={topic.thread_display_state} />}
        {menu && (
          <TopicMenu
            topic={topic}
            className="opacity-0 group-hover:opacity-100 focus-visible:opacity-100 data-[state=open]:opacity-100 [@media(hover:none)]:opacity-100"
          />
        )}
      </div>
      <TopicTags topic={topic} />
      {showPaths && <TopicPath topic={topic} />}
      <AttentionChips stats={topicAttention(topic)} />
      <span className="text-sm text-text-muted">
        <RelativeTime at={lastActivityOf(topic)} />
      </span>
    </>
  );
}

function Card({ topic, pending, canMove, showPaths, index, count, measure, style }: {
  topic: Topic; pending: boolean; canMove: boolean; showPaths: boolean;
  index: number; count: number; measure?: ((element: HTMLLIElement | null) => void) | undefined; style?: CSSProperties | undefined;
}) {
  // Waiting for the answer to a drag that was dropped, or to an act chosen from the card's own menu.
  const waiting = useIsPending(topic.id);
  const movable = canMove && holds(topic.access, 'topic.interact') && !topic.archived && !pending && !waiting;
  const drag = useDraggable({ id: topic.id, data: { column: columnOf(topic) }, disabled: !movable });
  // The card stays an item of its list, which a card that holds a link and a menu would stop being if it were
  // called a button as the library would have it. It can be picked up by the keys only where it can be moved.
  const { tabIndex, 'aria-roledescription': kind, 'aria-describedby': instructions } = drag.attributes;
  return (
    <li
      ref={(node) => { drag.setNodeRef(node); measure?.(node); }}
      data-topic={topic.id}
      data-index={index}
      aria-posinset={index + 1}
      aria-setsize={count}
      style={style}
      tabIndex={movable ? tabIndex : undefined}
      aria-roledescription={movable ? kind : undefined}
      aria-describedby={movable ? instructions : undefined}
      {...drag.listeners}
      className={cx(
        CARD,
        'hover:border-border-strong',
        movable && 'cursor-grab',
        (drag.isDragging || pending || waiting || topic.archived) && 'opacity-50',
      )}
    >
      <CardFace topic={topic} showPaths={showPaths} />
    </li>
  );
}

interface ColumnProps {
  showPaths: boolean;
  column: BoardColumn;
  topics: readonly Topic[];
  pendingIds: ReadonlySet<string>;
  canMove: boolean;
  /** The column of the card that is being carried, while one is. */
  dragging: BoardColumn | null;
  carriedId: string | null;
}

function ColumnView({ column, topics, pendingIds, canMove, dragging, carriedId, showPaths }: ColumnProps) {
  const { t, i18n } = useTranslation();
  const drop = useDroppable({ id: column });
  const [scroller, setScroller] = useState<HTMLDivElement | null>(null);
  const [focused, setFocused] = useState<string | null>(null);
  // A page remains ordinary flow; a long loaded column only mounts its visible cards.
  const windowed = topics.length > 50;
  const retained = [focused, carriedId].flatMap((id) => id === null ? [] : [topics.findIndex((topic) => topic.id === id)]).filter((index) => index >= 0);
  // The measured library owns positions; active drag sources and keyboard focus remain mounted.
  // eslint-disable-next-line react-hooks/incompatible-library
  const virtualizer = useVirtualizer({
    count: topics.length,
    enabled: windowed,
    getScrollElement: () => scroller,
    getItemKey: (index) => topics[index]?.id ?? index,
    estimateSize: () => 180,
    gap: 8,
    overscan: 8,
    initialRect: { width: 0, height: 384 },
    rangeExtractor: (range) => [...new Set([...defaultRangeExtractor(range), ...retained])].sort((one, other) => one - other),
  });
  const cards = windowed ? virtualizer.getVirtualItems().map((item) => ({ index: item.index, top: item.start })) : topics.map((_, index) => ({ index, top: 0 }));
  const quiet = HEADINGS[column];
  const heading = column === 'done' ? t('places.board.done') : t(`state.${column}`);
  // While a card is carried the columns it can be dropped in say so, and the others step back.
  const target = dragging !== null && statusOnDrop(dragging, column) !== null;
  return (
    <section
      ref={(node) => drop.setNodeRef(node)}
      aria-labelledby={`column-${column}`}
      className={cx(
        'flex min-w-0 flex-col gap-3 rounded-lg bg-bg-subtle p-3 transition-colors',
        target && 'outline-2 outline-dashed outline-accent',
        target && drop.isOver && 'bg-accent-subtle',
        dragging !== null && !target && 'opacity-50',
      )}
    >
      <h3 id={`column-${column}`} className="flex items-center gap-2 text-base font-medium text-text">
        {quiet === undefined ? <StateIcon state={column === 'done' ? 'done' : column} /> : <quiet.Icon className={quiet.tone} aria-hidden />}
        {heading}
        <span className="text-sm font-normal text-text-muted">{formatNumber(topics.length, i18n.resolvedLanguage ?? i18n.language)}</span>
      </h3>
      {topics.length === 0 ? (
        <EmptyState compact title={t('places.board.emptyColumn', { state: heading })} />
      ) : (
        <ScrollArea ref={setScroller} className="max-h-96">
          <ul aria-label={heading} className={windowed ? 'relative' : 'flex flex-col gap-2'} style={windowed ? { height: virtualizer.getTotalSize() } : undefined}
            onFocusCapture={(event) => { const item = event.target.closest<HTMLLIElement>('li[data-topic]'); if (item !== null) setFocused(item.dataset.topic ?? null); }}
            onBlurCapture={(event) => { if (!(event.relatedTarget instanceof Node) || !event.currentTarget.contains(event.relatedTarget)) setFocused(null); }}>
            {cards.map(({ index, top }) => {
              const topic = topics[index];
              if (topic === undefined) return null;
              return <Card key={topic.id} topic={topic} index={index} count={topics.length} pending={pendingIds.has(topic.id)} canMove={canMove} showPaths={showPaths}
                measure={windowed ? virtualizer.measureElement : undefined}
                style={windowed ? { position: 'absolute', top: 0, left: 0, width: '100%', transform: 'translateY(' + String(top) + 'px)' } : undefined} />;
            })}
          </ul>
        </ScrollArea>
      )}
    </section>
  );
}

export interface TopicBoardProps {
  showPaths?: boolean;
  topics: readonly Topic[];
  /** Whether the reader may set a topic's status, which is what dragging into or out of Done does. */
  canSetStatus: boolean;
  /** The root org of the project, whose tree is asked for what changed after a move. */
  orgId: string;
}

export function TopicBoard({ topics, canSetStatus, orgId: _orgId, showPaths = false }: TopicBoardProps) {
  const { t } = useTranslation();
  const describe = useErrorText();
  const changeStatus = useStatusChange();
  const [carried, setCarried] = useState<Topic | null>(null);
  const [pendingIds, setPendingIds] = useState<ReadonlySet<string>>(new Set());
  const sensors = useSensors(
    // A press that does not travel is a click on the card's link; a drag begins after a few pixels.
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
    useSensor(KeyboardSensor),
  );

  const setStatus = useMutation({
    mutationFn: ({ topic, status }: { topic: Topic; status: TopicStatus }) => changeStatus(topic, status),
    onMutate: ({ topic }) => setPendingIds((ids) => new Set(ids).add(topic.id)),
    onError: (error, { topic }) =>
      toast({
        tone: 'danger',
        title: t('places.toast.notChanged', { name: topic.name }),
        description: describe(error).message,
        action: { label: t('common.details'), run: () => openNodeDialog({ type: 'error', title: t('places.toast.notChanged', { name: topic.name }), error }) },
      }),
    onSettled: (_data, _error, { topic }) =>
      setPendingIds((ids) => {
        const next = new Set(ids);
        next.delete(topic.id);
        return next;
      }),
  });

  const byColumn = new Map<BoardColumn, Topic[]>(BOARD_COLUMNS.map((column) => [column, []]));
  for (const topic of topics) byColumn.get(columnOf(topic))?.push(topic);

  const nameOf = (id: string | number) => topics.find((topic) => topic.id === id)?.name ?? '';
  const columnName = (id: string | number) => id === 'done' ? t('places.board.done') : t(`state.${id as BoardColumn}`);

  // Keep a visible destination in place during a short status drag, even when more page content follows
  // the board. The person can still scroll the page explicitly to reach a column outside the view.
  return (
    <DndContext
      autoScroll={false}
      sensors={sensors}
      onDragStart={({ active }) => setCarried(topics.find((topic) => topic.id === active.id) ?? null)}
      onDragCancel={() => setCarried(null)}
      onDragEnd={({ active, over }: DragEndEvent) => {
        setCarried(null);
        const topic = topics.find((candidate) => candidate.id === active.id);
        if (topic === undefined || over === null) return;
        const status = statusOnDrop(columnOf(topic), over.id as BoardColumn);
        if (status !== null) setStatus.mutate({ topic, status });
      }}
      accessibility={{
        screenReaderInstructions: { draggable: t('places.board.drag.instructions') },
        announcements: {
          onDragStart: ({ active }) => t('places.board.drag.pickedUp', { name: nameOf(active.id) }),
          onDragOver: ({ over }) => (over === null ? undefined : t('places.board.drag.over', { column: columnName(over.id) })),
          onDragEnd: ({ over }) => (over === null ? t('places.board.drag.putBack') : t('places.board.drag.dropped', { column: columnName(over.id) })),
          onDragCancel: () => t('places.board.drag.putBack'),
        },
      }}
    >
      <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-4">
        {BOARD_COLUMNS.map((column) => (
          <ColumnView
            key={column}
            column={column}
            topics={byColumn.get(column) ?? []}
            pendingIds={pendingIds}
            canMove={canSetStatus}
            dragging={carried === null ? null : columnOf(carried)}
            carriedId={carried?.id ?? null}
            showPaths={showPaths}
          />
        ))}
      </div>
      <DragOverlay dropAnimation={null}>
        {carried !== null && (
          <div className={cx(CARD, 'cursor-grabbing border-border-strong shadow-md')}>
            <CardFace topic={carried} menu={false} showPaths={showPaths} />
          </div>
        )}
      </DragOverlay>
    </DndContext>
  );
}
