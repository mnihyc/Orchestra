/** Visible hierarchy and private ancestor references.
 *
 * The tree of a root org as the left bar draws it: the orgs inside it, however deep, with their
 * projects; the folders inside a project, which are projects that lie in a project and nest as deep
 * as the work does, standing above its topics; each topic's threads; and, where the reader wants
 * them, the documents written beside each of these and the chat of each, after what else it holds.
 * What stands where, and what each row carries of what it hides, is worked out in `treeRows.ts`;
 * this file draws the rows.
 *
 * A topic that is done stays where it is: its icon carries a check and its name is a shade quieter.
 * An org, a project, a folder and a topic show the glyph of their kind in a colour; a thread shows
 * what it is doing. A thread that has come further than its reader has read is bold with a dot, and
 * so is a row that hides something unread. A row carries a number only for what waits for the reader
 * in person: the decisions that wait, and the times they are mentioned. What a row carries stands in
 * one column at its end; the "+" and the "⋯" lie over that column where the pointer rests or the
 * keyboard is, and take no room of their own. A thing that not everybody may see shows a lock.
 *
 * While the tree scrolls, the places above its first row on screen stay at its top, so that a row is
 * always seen with what it lies in; a press on one of them brings it into view.
 *
 * A row leads to its screen, opens and closes, and shows its state. Its right-click menu, its "⋯" and
 * the "+" of an org, a project or a folder are the lists of `NodeMenu`, made only for the row that is
 * asked; a document's are a document's own.
 */
import { Link } from '@tanstack/react-router';
import type { TFunction } from 'i18next';
import { ChevronsDown, Copy, Ellipsis, ExternalLink, Focus, Lock, LoaderCircle, MessageSquare, MessagesSquare, RotateCw } from 'lucide-react';
import { cloneElement, createContext, type DragEvent, type KeyboardEvent, type ReactElement, type ReactNode, type RefObject, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';

import type { DocumentRef } from '@/api/documents';
import { isPlainClick, useOpenPath } from '@/features/chat/open';
import { DocContextMenu, DocMenuButton } from '@/features/navigation/DocMenu';
import { openNodeDialog } from '@/features/navigation/NodeDialogs';
import { NodeContextMenu, NodeMakeButton, NodeMenuButton, useIsPending } from '@/features/navigation/NodeMenu';
import { destinationsFor } from '@/features/org/Move';
import { kindWord, PlaceLink, type PlaceRef, placeOfRow, placePath } from '@/features/navigation/places';
import { loadMoreDocs, retryDocs } from '@/features/navigation/treeDocs';
import { buildRows, type ChatMark, type NavItem, nameOf, newLayoutMemory, type TreeView } from '@/features/navigation/treeRows';
import type { AdminAccess, TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';
import { Badge } from '@/ui/Badge';
import { AsAdminMark, adminAccessOf } from '@/ui/AsAdminMark';
import { copyText } from '@/ui/clipboard';
import { ContextMenu } from '@/ui/ContextMenu';
import { cx } from '@/ui/cx';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { NodeIcon, type NodeIconKind } from '@/ui/NodeIcon';
import { StateIcon } from '@/ui/StateChip';
import { ThreadStateIcon } from '@/ui/ThreadState';
import { RowEnd, RowPlace, type RowPlaceValue } from '@/ui/RowDetails';
import { StatusDot } from '@/ui/StatusDot';
import { toast } from '@/ui/Toast';
import { Tree, type TreeRow } from '@/ui/Tree';

export {
  buildRows,
  chatRowId,
  docRowId,
  firstExpanded,
  GROUPS_ROW,
  type LayoutMemory,
  type NavItem,
  newLayoutMemory,
  placesWithDocs,
  stopsInOrder,
  threadsInOrder,
  type TreeView,
} from '@/features/navigation/treeRows';

const ROW_LINK = 'flex min-w-0 flex-1 items-center gap-2 self-stretch outline-none';

/**
 * A name that is quieter than its neighbours: a topic that is done, a chat its reader does not follow. Quiet on the ground of
 * the bar; under the pointer and on the row of the screen in view, whose grounds are darker, it is as dark as any name, since
 * the quiet shade would no longer be read against them.
 */
const QUIET_NAME = 'text-text-subtle group-hover/row:text-text group-aria-[current=page]/row:text-text';

/** What a row carries, in one column at its end: the dot, the numbers, what goes on inside, the glyph of its chat, the lock. */
export const ROW_MARKS = 'flex shrink-0 items-center gap-1';

/**
 * The buttons of a row. They lie over the end of the row where the pointer rests or the keyboard is, on the row's own
 * ground, so that they take no room and move no text. The selected row also exposes its actions on touch screens.
 * What they lie over is what a row only says. The glyph of a place's chat is pressed, and stands last in the row: beside
 * it the buttons end where it begins.
 */
const rowButtons = (besideGlyph: boolean): string =>
  cx(
    // Set in by the width of the ring a row shows while the keyboard is on it, so that the ring is whole round them.
    'absolute inset-y-0.5 flex items-center opacity-0 transition-opacity',
    besideGlyph ? 'right-6 pr-1' : 'right-0.5',
    'bg-bg-subtle group-hover/row:bg-bg-muted group-aria-[current=page]/row:bg-accent-subtle',
    'group-hover/row:opacity-100 group-focus-within/row:opacity-100 group-aria-[current=page]/row:opacity-100',
  );
export const ROW_BUTTONS = rowButtons(false);
const ROW_BUTTONS_BESIDE_GLYPH = rowButtons(true);

/**
 * Where the chat of a place is: the place's own screen with its chat panel open. An org is named by its address,
 * which is where its screen is: named by its id it is sent on to that address, and what the address asked for is lost on the way.
 */
export const chatPath = (node: Pick<TreeNode, 'id' | 'kind' | 'address'>): string =>
  `${node.kind === 'org' && node.address !== null ? `/org/${node.address}` : placePath(node)}?panel=chat`;

/** A link to an address of the application that is written out as a path with what it asks for, such as a place with its chat panel. */
function PathLink({ path, className, label, children }: { path: string; className: string; label?: string; children: ReactNode }) {
  const open = useOpenPath();
  return (
    <a
      href={path}
      tabIndex={-1}
      className={className}
      {...(label === undefined ? {} : { 'aria-label': label })}
      onClick={(event) => {
        // With a modifier or another button the browser's own way with a link is wanted: a new tab.
        if (!isPlainClick(event)) return;
        event.preventDefault();
        open(path);
      }}
    >
      {children}
    </a>
  );
}

/** What fills a glyph with the colour it is drawn in. */
const FILLED = { fill: 'currentColor' } as const;

/**
 * The small glyph of a place's chat, which leads to it: an outline where the reader has read it, filled in the accent
 * where it is unread for them, and faint where they do not follow it.
 */
function ChatGlyph({ node, mark, mentions }: { node: TreeNode; mark: Exclude<ChatMark, 'none'>; mentions: number }) {
  const { t } = useTranslation();
  const label = mentions > 0 ? t('navigation.chat.glyphMentions', { count: mentions }) : t(`navigation.chat.glyph.${mark}`);
  return (
    <PathLink path={chatPath(node)} label={label} className="flex size-4 shrink-0 items-center justify-center rounded-sm hover:bg-border">
      <MessageSquare
        size={16}
        aria-hidden
        data-chat-glyph={mark}
        // Unread is told by the shape as well as by the colour.
        {...(mark === 'unread' ? FILLED : {})}
        // A chat that is not followed is a shade fainter, and still dark enough to be found and pressed.
        className={mark === 'unread' ? 'text-accent' : mark === 'quiet' ? 'text-text-subtle' : 'text-text-muted'}
      />
    </PathLink>
  );
}

/** What a row is carried as when it is dragged: the place, to be moved where it is dropped. */
const DRAGGED_PLACE = 'application/x-orch-place';

/** What a dropped row needs of the tree it is drawn in, read when it is dropped: the nodes, the rows of a parent in the order they stand, and the way to keep an order of one's own. */
interface TreeDrop {
  nodes: ReadonlyMap<string, TreeNode>;
  siblingsOf: (parentId: string) => string[];
  arrange: ((parentId: string, siblings: readonly string[], moved: string, before: string | null) => void) | undefined;
}

/** One object for the whole life of a tree, so that no row is drawn again for it; what it holds is the tree's latest. */
const TreeDropRef = createContext<RefObject<TreeDrop | null> | null>(null);

/** Flat rows use the same move-plan checks, without treating their time order as a sibling order. */
export function NavigationDropProvider({ nodes, children }: { nodes: ReadonlyMap<string, TreeNode>; children: ReactNode }) {
  const drop = useRef<TreeDrop | null>(null);
  useEffect(() => { drop.current = { nodes, siblingsOf: () => [], arrange: undefined }; }, [nodes]);
  return <TreeDropRef.Provider value={drop}>{children}</TreeDropRef.Provider>;
}

/**
 * What dropping a dragged place on this row does: a drop is never a move by itself. Between two rows of the same parent,
 * near the edge of one, it puts the row there and makes the reader's order their own, from the order they saw. On a
 * place that can hold it, Move to… opens with this place chosen and the whole plan shown; where it cannot go, the reason
 * is said and nothing is written.
 */
function useDropOnto(target: TreeNode) {
  const { t } = useTranslation();
  const held = useContext(TreeDropRef);
  const holds = target.kind === 'org' || target.kind === 'project';
  return {
    onDragOver: (event: DragEvent) => {
      if (!event.dataTransfer.types.includes(DRAGGED_PLACE)) return;
      if (!holds && held?.current?.arrange === undefined) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = 'move';
    },
    onDrop: (event: DragEvent) => {
      const carried = event.dataTransfer.getData(DRAGGED_PLACE);
      const drop = held?.current ?? null;
      if (carried === '' || drop === null) return;
      event.preventDefault();
      const dragged = JSON.parse(carried) as PlaceRef;
      if (dragged.id === target.id) return;
      // Within the same parent: the edges of a row that holds places, or anywhere on one that holds none, put it beside.
      const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
      const along = (event.clientY - box.top) / Math.max(1, box.height);
      const beside = !holds || along < 0.25 || along > 0.75;
      if (dragged.parentId === target.parent_id && target.parent_id !== null && beside && drop.arrange !== undefined) {
        const siblings = drop.siblingsOf(target.parent_id);
        const at = siblings.indexOf(target.id);
        const before = along > 0.5 ? (siblings[at + 1] ?? null) : target.id;
        drop.arrange(target.parent_id, siblings, dragged.id, before === dragged.id ? (siblings[at + 2] ?? null) : before);
        return;
      }
      if (!holds) {
        toast({ tone: 'info', title: t('places.move.dropRefused', { name: dragged.name, place: target.name }) });
        return;
      }
      const destination = destinationsFor(drop.nodes, dragged, t('places.pathJoin')).find((one) => one.id === target.id);
      if (destination === undefined) {
        toast({ tone: 'info', title: t('places.move.dropRefused', { name: dragged.name, place: target.name }) });
        return;
      }
      if (destination.why !== null) {
        toast({ tone: 'info', title: t(`places.move.why.${destination.why}`) });
        return;
      }
      openNodeDialog({ type: 'move', node: dragged, to: target.id });
    },
  };
}

export function NodeRow({ item, where, end, endTitle, extraEntries }: { item: Extract<NavItem, { type: 'node' }>; where?: RowPlaceValue | null; end?: ReactNode; endTitle?: string; extraEntries?: MenuEntry[] | undefined }) {
  const { node, urgent, folder, waiting, unread, mentions, chat, chatMentions } = item;
  const { t } = useTranslation();
  const pending = useIsPending(node.id);
  const place = placeOfRow(node, folder);
  const kind = kindWord(place);
  const holdsPlaces = node.kind === 'org' || node.kind === 'project';
  // A project, a folder, a topic and an org inside another can be dragged onto another place to be moved there.
  const movable = node.kind === 'project' || node.kind === 'topic' || (node.kind === 'org' && node.parent_id !== null);
  const drop = useDropOnto(node);
  const isThread = node.kind === 'thread';
  const done = node.topic?.status === 'done';
  // A thread's own icon says what it does; any other row says at its end what goes on in what it hides. What the number
  // of decisions that wait says already is not said a second time by a mark.
  const marked = !isThread && urgent !== null;
  return (
    <>
      <PlaceLink
        place={node}
        tabIndex={-1}
        className={cx(ROW_LINK, node.archived && 'opacity-60')}
        data-row-name-place={where !== undefined && where !== null || undefined}
        draggable={movable}
        onDragStart={(event) => {
          if (!movable) return;
          event.dataTransfer.setData(DRAGGED_PLACE, JSON.stringify(place));
          event.dataTransfer.effectAllowed = 'move';
        }}
        onDragOver={drop.onDragOver}
        onDrop={drop.onDrop}
      >
        <span className="flex size-4 shrink-0 items-center justify-center text-text-subtle">
          {isThread ? <ThreadStateIcon display={node.thread?.display} /> : <NodeIcon kind={kind} color={node.color} own={node.custom_color} done={done} />}
        </span>
        <span className={cx('truncate', unread && 'font-semibold text-text', done && !unread && QUIET_NAME)}>{node.name}</span>
        {where !== undefined && <RowPlace value={where} />}
        {!isThread && <span className="sr-only">{t(done ? 'navigation.row.topicDone' : (`navigation.row.${kind}` as 'navigation.row.org'))}</span>}
      </PlaceLink>
      <span className={ROW_MARKS}>
        <AsAdminMark of={adminAccessOf(node)?.place} iconOnly />
        {pending && <LoaderCircle className="animate-spin text-text-subtle" aria-label={t('places.menu.working')} role="img" />}
        {/* A number of mentions says that something is unread already: the dot is for when there is none. */}
        {unread && urgent !== 'finished' && mentions === 0 && <StatusDot tone="accent" label={t(isThread ? 'navigation.unread' : 'navigation.unreadInside')} />}
        {mentions > 0 && <Badge tone="accent" count={mentions} label={t('navigation.mentionsInside', { count: mentions })} />}
        {waiting > 0 && <Badge tone="warning" count={waiting} label={t(isThread ? 'navigation.approvalsWaiting' : 'places.tree.approvalsInside', { count: waiting })} />}
        {!isThread && node.archived && <StateIcon state="archived" />}
        {marked && <StateIcon state={urgent} label={t(`navigation.holds.${urgent}` as 'navigation.holds.blocked')} />}
        {node.restricted && (
          <span role="img" aria-label={t('navigation.restricted')} className="flex shrink-0 text-text-subtle">
            <Lock size={16} aria-hidden />
          </span>
        )}
        {/* Last, where the row's buttons do not reach: it is the one mark that is pressed. */}
        {chat !== 'none' && chatMentions > 0 && <Badge tone="accent" count={chatMentions} />}
        {chat !== 'none' && <ChatGlyph node={node} mark={chat} mentions={chatMentions} />}
      </span>
      {end === undefined ? <span className={chat === 'none' ? ROW_BUTTONS : ROW_BUTTONS_BESIDE_GLYPH}>
        {holdsPlaces && <NodeMakeButton place={place} skipTab />}
        <NodeMenuButton place={place} skipTab extraEntries={extraEntries} />
      </span> : <RowEnd time={end} title={endTitle}>
        {holdsPlaces && <NodeMakeButton place={place} skipTab />}
        <NodeMenuButton place={place} skipTab extraEntries={extraEntries} />
      </RowEnd>}
    </>
  );
}

/** A document written beside a place: its glyph and its title, bold while it holds something its reader has not read. */
function DocRow({ doc, me }: { doc: DocumentRef; me: string | null }) {
  const { t } = useTranslation();
  const by = doc.updated_by;
  const mine = by !== null && 'id' in by && by.id === me;
  const changed = !mine && (doc.seen_seq === null || doc.seq > doc.seen_seq);
  return (
    <>
      <Link to="/doc/$documentId" params={{ documentId: doc.id }} tabIndex={-1} className={ROW_LINK}>
        <span className="flex size-4 shrink-0 items-center justify-center">
          {/* The glyph says what the row is, before its title, as a thread's icon says what it is doing before its name. */}
          <NodeIcon kind="document" label={t(changed ? 'navigation.row.documentChanged' : 'navigation.row.document')} />
        </span>
        <span className={cx('truncate', changed && 'font-semibold text-text')}>{doc.title}</span>
      </Link>
      <span className={ROW_MARKS}>
        <AsAdminMark of={adminAccessOf(doc)?.place} iconOnly />
        {doc.page === 'restricted' && (
          <span role="img" aria-label={t('navigation.privateDocument')} className="flex shrink-0 text-text-subtle">
            <Lock size={16} aria-hidden />
          </span>
        )}
      </span>
      <span className={ROW_BUTTONS}>
        <DocMenuButton doc={doc} skipTab />
      </span>
    </>
  );
}

/** The line under a place's documents: that they are on their way, that they could not be read, or that there are more. */
function DocsRow({ nodeId, state }: { nodeId: string; state: 'loading' | 'failed' | 'more' | 'loadingMore' }) {
  const { t } = useTranslation();
  if (state === 'loading' || state === 'loadingMore') {
    return (
      <span className="flex min-w-0 flex-1 items-center gap-2 text-text-muted">
        <LoaderCircle className="animate-spin" aria-hidden />
        <span className="truncate">{t('navigation.docs.loading')}</span>
      </span>
    );
  }
  return (
    <button
      type="button"
      tabIndex={-1}
      className="flex min-w-0 flex-1 items-center gap-2 self-stretch text-left text-text-muted outline-none hover:text-text"
      onClick={() => (state === 'failed' ? retryDocs(nodeId) : loadMoreDocs(nodeId))}
    >
      <span aria-hidden className="flex size-4 shrink-0 items-center justify-center text-text-subtle">
        {state === 'failed' ? <RotateCw /> : <ChevronsDown />}
      </span>
      <span className="truncate underline underline-offset-2">{t(state === 'failed' ? 'navigation.docs.retry' : 'navigation.docs.more')}</span>
    </button>
  );
}

/** The list of a chat's row: where it leads, and its link. */
function chatEntries(path: string, t: TFunction, open: (path: string) => void): MenuEntry[] {
  const copy = () => copyText(new URL(path, location.origin).href, t('common.linkCopied'));
  return [
    { id: 'open', label: t('places.menu.open'), onSelect: () => open(path) },
    { id: 'open-tab', label: t('places.menu.openInTab'), icon: <ExternalLink aria-hidden />, shortcut: 'Mod+Enter', onSelect: () => window.open(path, '_blank', 'noopener') },
    { type: 'separator', id: 'separator-link' },
    { id: 'copy-link', label: t('places.menu.copyLink'), icon: <Copy aria-hidden />, onSelect: () => void copy() },
  ];
}

/** A chat's row: its glyph, its name, what says it is unread, and its menu. A chat its reader does not follow is quiet, and still says when they are mentioned. */
function ChatRow({ name, of, path, unread, mentions, quiet, adminAccess }: { name: string; of: string; path: string; unread: boolean; mentions: number; quiet: boolean; adminAccess?: AdminAccess | null }) {
  const { t } = useTranslation();
  const open = useOpenPath();
  return (
    <>
      <PathLink path={path} className={ROW_LINK}>
        <span className="flex size-4 shrink-0 items-center justify-center text-text-subtle">
          <MessageSquare aria-hidden />
        </span>
        <span className={cx('truncate', unread && 'font-semibold text-text', quiet && QUIET_NAME)}>{name}</span>
        {quiet && <span className="sr-only">{t('navigation.chat.notFollowed')}</span>}
      </PathLink>
      <span className={ROW_MARKS}>
        <AsAdminMark of={adminAccess?.place} iconOnly />
        {mentions > 0 ? <Badge tone="accent" count={mentions} label={t('navigation.chat.mentions', { count: mentions })} /> : unread ? <StatusDot tone="accent" label={t('navigation.unread')} /> : null}
      </span>
      <span className={ROW_BUTTONS}>
        <Menu
          label={t('places.menu.of', { name: of })}
          align="end"
          entries={chatEntries(path, t, open)}
          trigger={<IconButton size="sm" label={t('places.menu.more', { name: of })} icon={<Ellipsis aria-hidden />} tabIndex={-1} />}
        />
      </span>
    </>
  );
}

/** Gives a chat's row its right-click menu: the same list as its "⋯". */
function ChatContextMenu({ of, path, children }: { of: string; path: string; children: ReactElement }) {
  const { t } = useTranslation();
  const open = useOpenPath();
  return (
    <ContextMenu label={t('places.menu.of', { name: of })} entries={chatEntries(path, t, open)}>
      {children}
    </ContextMenu>
  );
}

/**
 * The first line of a narrowed tree while the thing in view lies outside it: where the thing lies, and the way to see it
 * there. With room it is said in full; in a narrow bar it is the place the thing lies in, since the thing itself is what
 * the main area shows. A screen reader is told in full either way.
 */
function OutsideRow({ node, where, place, onShow }: { node: TreeNode; where: string; place: string; onShow: () => void }) {
  const { t } = useTranslation();
  const whole = t('navigation.outside.lies', { name: node.name, where });
  return (
    <>
      <span className="flex min-w-0 flex-1 items-center gap-2 text-sm text-text-muted">
        <span aria-hidden className="size-4 shrink-0" />
        <span className="sr-only">{whole}</span>
        <span aria-hidden className="truncate @[24rem]:hidden">
          {t('navigation.outside.short', { place })}
        </span>
        <span aria-hidden className="hidden truncate @[24rem]:block">
          {whole}
        </span>
      </span>
      <button type="button" tabIndex={-1} className="flex shrink-0 items-center gap-1 rounded-sm px-1 text-sm text-accent-text outline-none hover:text-text" onClick={onShow}>
        <Focus aria-hidden size={16} />
        <span className="underline underline-offset-2">{t('navigation.outside.show')}</span>
      </button>
    </>
  );
}

// ---------------------------------------------------------------- the places that stay

/** The most of the tree's height the places that stay may take: a third, so that two thirds are always rows that scroll. */
const STAYING_SHARE = 1 / 3;

/** How many levels a place that stays is set in by: the levels a row of the tree is set in by a whole step. */
const STAYING_STEPS = 6;

interface Staying {
  id: string;
  depth: number;
  name: string;
  icon: ReactNode;
}

/**
 * The places above the first row on screen, outermost first, for as many as fit in a third of what scrolls. They are read
 * from where the tree stands in what scrolls it: every row has one height, so the row at the top edge is known by a division.
 */
function useStaying(rows: readonly TreeRow<NavItem>[], iconOf: (item: NavItem) => ReactNode): { anchor: (element: HTMLDivElement | null) => void; staying: readonly Staying[] } {
  const [first, setFirst] = useState<{ index: number; room: number }>({ index: 0, room: 0 });
  const [anchor, setAnchor] = useState<HTMLDivElement | null>(null);
  useEffect(() => {
    const scroller = anchor?.closest<HTMLElement>('[data-bar-scroll]') ?? null;
    if (anchor === null || scroller === null) return;
    let frame = 0;
    const read = () => {
      frame = 0;
      const row = anchor.parentElement?.querySelector<HTMLElement>('[role="treeitem"]');
      const height = row?.offsetHeight ?? 0;
      if (height === 0) return;
      // How far the top edge of what scrolls lies below the top of the tree, in rows.
      const above = scroller.getBoundingClientRect().top - (anchor.parentElement?.getBoundingClientRect().top ?? 0);
      const index = above <= 0 ? 0 : Math.floor(above / height);
      const room = Math.floor((scroller.clientHeight * STAYING_SHARE) / height);
      setFirst((held) => (held.index === index && held.room === room ? held : { index, room }));
    };
    const onScroll = () => {
      if (frame === 0) frame = requestAnimationFrame(read);
    };
    onScroll();
    scroller.addEventListener('scroll', onScroll, { passive: true });
    window.addEventListener('resize', onScroll);
    return () => {
      scroller.removeEventListener('scroll', onScroll);
      window.removeEventListener('resize', onScroll);
      if (frame !== 0) cancelAnimationFrame(frame);
    };
  }, [anchor]);

  const staying = useMemo(() => {
    if (first.index === 0 || first.room === 0) return [];
    const byId = new Map(rows.map((row) => [row.id, row]));
    const places = (index: number): TreeRow<NavItem>[] => {
      const found: TreeRow<NavItem>[] = [];
      let at = rows[Math.min(index, rows.length - 1)]?.parentId ?? null;
      for (let steps = 0; at !== null && steps <= rows.length; steps += 1) {
        const row = byId.get(at);
        if (row === undefined) break;
        found.unshift(row);
        at = row.parentId;
      }
      return found;
    };
    let above = places(first.index);
    // What stays covers as many rows as it has: the places of the first row that is still seen are the ones to show.
    for (let pass = 0; pass < first.room; pass += 1) {
      const next = places(first.index + Math.min(above.length, first.room));
      if (next.length === above.length) break;
      above = next;
    }
    // More places than there is room for: the nearest ones stay, since the far ones are read in the heading of the tree.
    return above.slice(-first.room).map((row): Staying => ({ id: row.id, depth: row.depth, name: nameOf(row.item), icon: iconOf(row.item) }));
  }, [rows, first, iconOf]);
  return { anchor: setAnchor, staying };
}

/** The icon of a row, for where the row is drawn again as a place that stays. */
function iconOf(item: NavItem): ReactNode {
  if (item.type === 'private') return <Lock aria-hidden className="text-text-subtle" />;
  if (item.type !== 'node') return <MessagesSquare aria-hidden className="text-text-subtle" />;
  if (item.node.kind === 'thread') return <ThreadStateIcon display={item.node.thread?.display} />;
  const kind: NodeIconKind = item.node.kind === 'project' && item.folder ? 'folder' : item.node.kind;
  return <NodeIcon kind={kind} color={item.node.color} own={item.node.custom_color} done={item.node.topic?.status === 'done'} />;
}

/** Brings a row of the tree to the top of what scrolls, and gives it the keyboard. */
function bring(id: string): void {
  const row = document.querySelector<HTMLElement>(`[data-tree-row="${CSS.escape(id)}"]`);
  row?.scrollIntoView({ block: 'start' });
  row?.focus({ preventScroll: true });
}

export interface NodeTreeProps {
  tree: OrgTree;
  /** The place the tree begins at: the root org, or the place its reader narrowed it to. */
  rootId: string;
  /** What the reader chose to see and what they have read. It must be the same object from one drawing to the next while nothing in it changed. */
  view: TreeView;
  /** The ids of the rows that are open. */
  expanded: ReadonlySet<string>;
  onExpandedChange: (id: string, expanded: boolean) => void;
  /** The node of the screen in view. */
  currentId: string | null;
  /** The row of the screen in view where it is no node's: a document's. Where that row is not in the tree, the node's is marked. */
  currentRow?: string | null;
  locale: string;
  /** The person who reads, so that a document they changed last themself is not marked as changed. */
  me: string | null;
  /** The short name of the root org, which the address of its group chats begins with. */
  orgSlug?: string | null;
  /** Shows the thing in view where it lies, when the tree was narrowed to a place it lies outside of. */
  onShowThere?: () => void;
  /** What stands in place of the tree when it has no row: that nothing has the typed letters in its name. */
  nothing?: string;
  /** Keeps the reader's own order of a parent's rows, where a row was dropped beside another: left out, rows are not arranged by hand. */
  onArrange?: (parentId: string, siblings: readonly string[], moved: string, before: string | null) => void;
}

export function NodeTree({ tree, rootId, view, expanded, onExpandedChange, currentId, currentRow = null, locale, me, orgSlug = null, onShowThere, nothing, onArrange }: NodeTreeProps) {
  const { t } = useTranslation();
  // What the last laying out was made from is kept: a row whose item did not change is not drawn again, and nodes that
  // did not move are not sorted again.
  const [memory] = useState(newLayoutMemory);
  const rows = useMemo(() => buildRows(tree, rootId, expanded, locale, view, memory), [tree, rootId, expanded, locale, view, memory]);
  // What a drop reads of the tree, the latest after each drawing, without drawing a row again for it.
  const drop = useRef<TreeDrop | null>(null);
  useEffect(() => {
    drop.current = {
      nodes: tree.nodes,
      siblingsOf: (parentId) => rows.filter((row) => row.parentId === parentId && row.item.type === 'node').map((row) => row.id),
      arrange: onArrange,
    };
  });
  const groupPath = useCallback((chatId: string) => `/org/${orgSlug ?? rootId}/messages?chat=${chatId}`, [orgSlug, rootId]);
  // The latest of what shows the thing in view where it lies, behind one function that stays the same.
  const showThere = useRef(onShowThere);
  useEffect(() => {
    showThere.current = onShowThere;
  }, [onShowThere]);
  const onShow = useCallback(() => showThere.current?.(), []);

  const renderRow = useCallback(
    (row: TreeRow<NavItem>) => {
      const item = row.item;
      switch (item.type) {
        case 'node':
          return <NodeRow item={item} />;
        case 'doc':
          return <DocRow doc={item.doc} me={me} />;
        case 'docs':
          return <DocsRow nodeId={item.nodeId} state={item.state} />;
        case 'chat':
          return (
            <ChatRow
              // The chat of the root org is the team's, and is called so wherever it stands.
              name={t(item.node.parent_id === null ? 'navigation.teamChat' : 'navigation.chat.name')}
              of={t('navigation.chat.of', { name: item.node.name })}
              path={chatPath(item.node)}
              adminAccess={adminAccessOf(item.node.chat)}
              unread={item.unread}
              mentions={item.mentions}
              quiet={item.quiet}
            />
          );
        case 'groups':
          return (
            <>
              <span className="flex min-w-0 flex-1 items-center gap-2">
                <span className="flex size-4 shrink-0 items-center justify-center text-text-subtle">
                  <MessagesSquare aria-hidden />
                </span>
                <span className={cx('truncate', item.unread && 'font-semibold text-text')}>{t('navigation.chat.groups')}</span>
              </span>
              <span className={ROW_MARKS}>
                {item.unread && <StatusDot tone="accent" label={t('navigation.unreadInside')} />}
                {item.mentions > 0 && <Badge tone="accent" count={item.mentions} label={t('navigation.mentionsInside', { count: item.mentions })} />}
              </span>
            </>
          );
        case 'group': {
          const name = item.chat.name ?? t('navigation.chat.group');
          return <ChatRow name={name} of={name} path={groupPath(item.chat.id)} unread={item.unread} mentions={item.mentions} quiet={item.quiet} adminAccess={adminAccessOf(item.chat)} />;
        }
        case 'outside':
          return <OutsideRow node={item.node} where={item.where} place={item.place} onShow={onShow} />;
        case 'private':
          return null;
      }
    },
    [me, t, groupPath, onShow],
  );
  // The whole row answers a right click, not only what it holds.
  const wrapRow = useCallback(
    (row: TreeRow<NavItem>, element: ReactElement) => {
      const item = row.item;
      // A line that is no link does on Enter and on the space bar what a press on it does.
      const pressed = (act: () => void) =>
        cloneElement(element as ReactElement<{ onKeyDown?: (event: KeyboardEvent<HTMLElement>) => void }>, {
          onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
            if (event.target !== event.currentTarget || (event.key !== 'Enter' && event.key !== ' ')) return;
            event.preventDefault();
            event.stopPropagation();
            act();
          },
        });
      switch (item.type) {
        case 'node':
          return <NodeContextMenu place={placeOfRow(item.node, item.folder)}>{element}</NodeContextMenu>;
        case 'doc':
          return <DocContextMenu doc={item.doc}>{element}</DocContextMenu>;
        case 'chat':
          return (
            <ChatContextMenu of={t('navigation.chat.of', { name: item.node.name })} path={chatPath(item.node)}>
              {element}
            </ChatContextMenu>
          );
        case 'group':
          return (
            <ChatContextMenu of={item.chat.name ?? t('navigation.chat.group')} path={groupPath(item.chat.id)}>
              {element}
            </ChatContextMenu>
          );
        case 'docs':
          return item.state === 'more' ? pressed(() => loadMoreDocs(item.nodeId)) : item.state === 'failed' ? pressed(() => retryDocs(item.nodeId)) : element;
        case 'outside':
          return pressed(onShow);
        case 'groups':
        case 'private':
          return element;
      }
    },
    [t, groupPath, onShow],
  );
  // The row of the screen in view is the document's own where it stands in the tree, and its place's where it does not.
  const current = useMemo(() => (currentRow !== null && rows.some((row) => row.id === currentRow) ? currentRow : currentId), [currentRow, currentId, rows]);
  const { anchor, staying } = useStaying(rows, iconOf);

  if (rows.length === 0 && nothing !== undefined) {
    return (
      <p role="status" className="flex min-h-(--row-height) items-center pr-3 pl-6 text-sm text-text-subtle">
        {nothing}
      </p>
    );
  }
  return (
    // Its own width is what a row measures its room by.
    <div className="@container relative flex flex-col">
      {/* It takes no room where it stands: it lies over the first rows, which are rows of the places it shows. */}
      <div ref={anchor} aria-hidden className="sticky top-0 z-(--layer-sticky) h-0">
        {staying.length > 0 && (
          <div data-staying className="absolute inset-x-0 top-0 flex flex-col border-b border-border bg-bg-subtle shadow-sm">
            {staying.map((place) => (
              <button
                key={place.id}
                type="button"
                tabIndex={-1}
                className="flex h-(--row-height) items-center gap-2 pr-2 text-left text-base text-text-muted hover:bg-bg-muted hover:text-text"
                // The same step for each level as a row of the tree has, after the room of its arrow.
                style={{ paddingInlineStart: `calc(var(--spacing-4) * ${Math.min(place.depth, STAYING_STEPS)} + var(--spacing-6))` }}
                onClick={() => bring(place.id)}
              >
                <span className="flex size-4 shrink-0 items-center justify-center">{place.icon}</span>
                <span className="truncate">{place.name}</span>
              </button>
            ))}
          </div>
        )}
      </div>
      <TreeDropRef.Provider value={drop}>
        <Tree label={t('navigation.tree')} rows={rows} currentId={current} onExpandedChange={onExpandedChange} nameOf={nameOf} renderRow={renderRow} wrapRow={wrapRow} />
      </TreeDropRef.Provider>
    </div>
  );
}
