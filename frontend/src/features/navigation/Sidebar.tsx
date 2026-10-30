/** The orgs inside the selected org, projects, topics, threads, starred and unread state, chat signals and Messages.
 *
 * The Navigation view of the left bar, from the outside in: the org in view with its own menu; the
 * org's own rows, its team's chat and its Messages; what its reader starred; what they opened last;
 * and the tree of the org. Inbox and Approvals are a person's and stand in the rail, with the orgs.
 *
 * Starred, Recent and Projects each fold, and how each stands is kept with the person on every
 * device. Folded, a section says in its heading only what waits inside: a dot while something its
 * reader follows is unread, and a number for the decisions that wait and the times they are mentioned.
 * The headings have the height of a row and the sections the same room between them, and every row
 * begins its icon on the same line as the rows of the tree.
 *
 * The tree is kept current by the org's live feed. Which of its rows are open, and where it begins,
 * are remembered on the device for each person and each org. A reader narrows the tree to one place
 * ("Show only this" in a place's menu, "Show only where I am" in the tree's own); the heading then
 * reads as the way to that place, each place above it a step back, and the tree stays there wherever
 * they go: while they look at something outside it, the first row says where that lies and offers to
 * show it there. The menu beside the heading is the tree's own: which kinds of thing stand in it
 * (threads, documents, chats), whether topics that are done and things that are archived are shown,
 * where it begins, and opening and closing its rows. Nothing in it folds or moves by itself: going
 * somewhere opens the way to the row once, and a reader who closes it again finds it closed.
 */
import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { Link, useNavigate, useRouterState } from '@tanstack/react-router';
import { ArrowUp, ChevronsDownUp, Ellipsis, Focus, FolderOpen, ListTree, Locate, MessagesSquare } from 'lucide-react';
import type { TFunction } from 'i18next';
import { Fragment, type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { choiceOption } from '@/i18n/choices';
import { useStore } from 'zustand';

import { orgChatsQuery } from '@/api/chats';
import { useParticipationCopy } from '@/features/access/ParticipationFilter';
import { client } from '@/api/client';
import { orgsQuery } from '@/api/nodes';
import { meQuery } from '@/api/auth';
import { markRead as markThreadRead } from '@/api/threads';
import { read as readChat } from '@/features/chat/acts';
import { useOpenPath } from '@/features/chat/open';
import { type Command, useRegisterCommands } from '@/features/finder/commands';
import { useShowArchived } from '@/features/navigation/archived';
import { attentionWriter, isUnread, reloadAttention, useAttention, useAttentionFeed } from '@/features/navigation/attention';
import {
  beginningOf,
  liesIn,
  othersFolded,
  placeAbove,
  projectOf,
  readBeginning,
  readOpen,
  useTreeActs,
  wayOpened,
  placesTo,
  whereIAm,
  writeBeginning,
  writeOpen,
} from '@/features/navigation/beginning';
import { useUnreadChats } from '@/features/navigation/counts';
import { useNavigationOrg } from '@/features/navigation/currentOrg';
import { DocContextMenu, DocMenuButton } from '@/features/navigation/DocMenu';
import { useFavorites } from '@/features/navigation/favorites';
import { NodeContextMenu, NodeMakeButton, NodeMenuButton } from '@/features/navigation/NodeMenu';
import { chatPath, NodeTree, ROW_BUTTONS, ROW_MARKS } from '@/features/navigation/NodeTree';
import { OrgSwitcher } from '@/features/navigation/OrgSwitcher';
import { TeamChatShortcut } from '@/features/navigation/TeamChat';
import { type DocPlace, kindWord, PlaceLink, type PlaceRef, placeOfNode, placeOfRow } from '@/features/navigation/places';
import { useSidebarPreferences } from '@/features/navigation/preferences';
import { arranged, type OrderChoice, useTreeOrder } from '@/features/navigation/treeOrder';
import { Section } from '@/features/navigation/Section';
import { TreeFilter } from '@/features/navigation/TreeFilter';
import { ArrangementSwitch, RecentOrderSelect } from './ArrangementSwitch';
import { navigationFilter, type NavigationArrangement, type RecentOrder } from './recent';
import { RecentList } from './RecentList';
import { useTreeReveal } from './showInTree';
import { useTreeDocsOf, loadMoreDocs } from '@/features/navigation/treeDocs';
import {
  type Below,
  belowOf,
  chatRowId,
  type ChatStanding,
  chatStanding,
  docRowId,
  groupRowId,
  GROUPS_ROW,
  firstExpanded,
  type GroupChat,
  type Needs as SectionNeeds,
  needsOf,
  placesWithDocs,
  stopsInOrder,
  type TreeView,
} from '@/features/navigation/treeRows';
import { RECENT_ROWS, recentsOfOrgQuery, useSeenHere } from '@/features/navigation/visits';
import { useShape } from '@/features/workbench/Narrow';
import type { EverythingRow, NodeKind, TreeNode } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { readViewChoice, writeViewChoice, useViewChoice } from '@/lib/viewChoices';
import { holds } from '@/lib/permissions';
import { reloadOrgTree, useOrgTreeFeed } from '@/realtime/live';
import { useChatStates } from '@/realtime/stores/chat';
import { pathTo, useOrgTree } from '@/realtime/stores/tree';
import { useActionKeys, useKeyAction } from '@/shell/keys';
import { railOrgs, selectOrg, useSelectedOrg } from '@/shell/org';
import { useCurrentHeading } from '@/shell/screen';
import { Badge } from '@/ui/Badge';
import { cx } from '@/ui/cx';
import { EmptyState } from '@/ui/EmptyState';
import { ErrorState } from '@/ui/ErrorState';
import { IconButton } from '@/ui/IconButton';
import { Menu, type MenuEntry } from '@/ui/Menu';
import { NodeIcon } from '@/ui/NodeIcon';
import { SkeletonRows } from '@/ui/Skeleton';
import { StatusDot } from '@/ui/StatusDot';
import { toast } from '@/ui/Toast';
import type { ViewMenuProps } from '@/features/workbench/views';
import { useViewShown } from '@/features/workbench/viewOpening';
import { nearestPlace, RowPlace, type RowPlaceValue } from '@/ui/RowDetails';

/**
 * A row of the bar that is no row of the tree. It begins where a row of the tree begins its icon: after
 * the room the tree's arrows take, so that icons and names stand on one line from the top of the bar to its foot.
 */
const ROW = 'group/row relative mx-1 flex h-(--row-height) items-center rounded-md pr-1 text-base';
const ROW_LINK = 'focus-inset flex min-w-0 flex-1 items-center gap-2 self-stretch pl-6';
const QUIET = 'text-text-muted transition-colors hover:bg-bg-muted hover:text-text';

/**
 * How many requests that mark something read are under way at once: browsers open at most six
 * connections to one host, so more would only wait in the browser's own queue.
 */
const READ_AT_ONCE = 6;
/** A short outline identifies a revealed row without becoming a second selection. */
const REVEAL_MS = 1600;

/** One of the org's own rows: its icon, its name, and at its end what waits there. */
function OrgRow({ icon, bold = false, marks, children }: { icon: ReactNode; bold?: boolean; marks?: ReactNode; children: ReactNode }) {
  return (
    <>
      <span className="flex size-4 shrink-0 items-center justify-center text-text-subtle">{icon}</span>
      <span className={cx('min-w-0 flex-1 truncate', bold && 'font-semibold text-text')}>{children}</span>
      {marks !== undefined && <span className={cx(ROW_MARKS, 'pr-1')}>{marks}</span>}
    </>
  );
}

const ORG_ROW = cx(ROW, 'focus-inset gap-2 pl-6', QUIET, 'aria-[current=page]:bg-accent-subtle aria-[current=page]:text-text');

/** What a shortcut carries of what it holds: a dot while something in it is unread, and the numbers of what waits for the reader in person. */
function Needs({ needs }: { needs: SectionNeeds }) {
  const { t } = useTranslation();
  return (
    <>
      {needs.unread && needs.mentions === 0 && <StatusDot tone="accent" label={t('navigation.unreadInside')} />}
      <Badge tone="accent" count={needs.mentions} label={t('navigation.mentionsInside', { count: needs.mentions })} />
      <Badge tone="warning" count={needs.waiting} label={t('places.tree.approvalsInside', { count: needs.waiting })} />
    </>
  );
}

/** One place the person starred or opened lately: its icon, its name, and the place it lies in, quietly. */
function ShortcutRow({ place, color, where, needs }: { place: PlaceRef; color: number | null; where: RowPlaceValue | null; needs: SectionNeeds }) {
  const { t } = useTranslation();
  const kind = kindWord(place);
  return (
    <NodeContextMenu place={place}>
      <div className={cx(ROW, QUIET)} data-shortcut>
        <PlaceLink place={place} className={ROW_LINK} data-row-name-place={where !== null || undefined}>
          <span className="flex size-4 shrink-0 items-center justify-center">
            <NodeIcon kind={kind} color={color} done={place.status === 'done'} />
          </span>
          <span className={cx('truncate', needs.unread && 'font-semibold text-text')}>{place.name}</span>
          <span className="sr-only">{t(`navigation.row.${kind}` as 'navigation.row.org')}</span>
          <RowPlace value={where} />
        </PlaceLink>
        <span className={ROW_MARKS}>
          <Needs needs={needs} />
        </span>
        <span className={ROW_BUTTONS}>
          <NodeMenuButton place={place} skipTab />
        </span>
      </div>
    </NodeContextMenu>
  );
}

/** A document the person opened lately. */
function DocumentShortcut({ doc, where }: { doc: DocPlace; where: RowPlaceValue | null }) {
  const { t } = useTranslation();
  return (
    <DocContextMenu doc={doc}>
      <div className={cx(ROW, QUIET)}>
        <Link to="/doc/$documentId" params={{ documentId: doc.id }} className={ROW_LINK} data-row-name-place={where !== null || undefined}>
          <span className="flex size-4 shrink-0 items-center justify-center">
            <NodeIcon kind="document" label={t('navigation.row.document')} />
          </span>
          <span className="truncate">{doc.title}</span>
          <RowPlace value={where} />
        </Link>
        <span className={ROW_BUTTONS}>
          <DocMenuButton doc={doc} skipTab />
        </span>
      </div>
    </DocContextMenu>
  );
}

/** One quiet line where a section has no row. */
function Nothing({ children }: { children: ReactNode }) {
  return <p className="flex h-(--row-height) items-center truncate pr-3 pl-6 text-sm text-text-subtle">{children}</p>;
}

/**
 * The heading of a tree that was narrowed to a place: the way to that place. The step up comes first;
 * with room, every place above is named and narrows the tree to itself, the org showing everything
 * again; the place the tree begins at is named last, and in a narrow bar it is named alone.
 */
function Beginning({ way, onBegin, onUp }: { way: readonly TreeNode[]; onBegin: (nodeId: string | null) => void; onUp: () => void }) {
  const { t } = useTranslation();
  const upKeys = useActionKeys('tree-up');
  const last = way.at(-1);
  if (last === undefined) return null;
  const kind = last.kind === 'project' && way.at(-2)?.kind === 'project' ? 'folder' : last.kind === 'thread' ? 'topic' : last.kind;
  return (
    <div role="group" aria-label={t('navigation.begins.label')} className="flex min-w-0 flex-1 items-center gap-1 text-sm">
      <IconButton size="sm" label={t('navigation.begins.up')} shortcut={upKeys} icon={<ArrowUp aria-hidden />} onClick={onUp} />
      {way.slice(0, -1).map((place, index) => (
        <Fragment key={place.id}>
          <button
            type="button"
            className="focus-inset hidden max-w-24 min-w-0 shrink truncate rounded-sm text-text-subtle transition-colors hover:text-text @[20rem]:block"
            aria-label={t('navigation.begins.at', { name: place.name })}
            onClick={() => onBegin(index === 0 ? null : place.id)}
          >
            {place.name}
          </button>
          <span aria-hidden className="hidden text-text-disabled @[20rem]:block">
            {t('navigation.pathJoin').trim()}
          </span>
        </Fragment>
      ))}
      <span data-tree-begins={last.id} className="flex min-w-0 items-center gap-1 font-medium text-text">
        <NodeIcon kind={kind} color={last.color} own={last.custom_color} done={last.topic?.status === 'done'} />
        <span className="truncate">{last.name}</span>
      </span>
    </div>
  );
}

/*
 * The keys that move the tree's beginning while the keyboard is in the tree (one place up, and to the row the keyboard
 * is on) and the one that gives the keyboard to the field that narrows the tree are the actions `tree-up`, `tree-only`
 * and `tree-filter` (`shell/keyActions.ts`): left and right with Alt alone are the browser's own way back and forward,
 * with Shift as well they are nobody's, and Ctrl+Alt+F is no browser's own on any system.
 */

/**
 * The keys that are the tree's own, bound for as long as this is drawn: which is while the keyboard is in the bar. The list
 * of keys is made from what is bound, so these are listed there to whoever asks from the tree, and not to somebody whose
 * keyboard is elsewhere, for whom they would do nothing.
 */
function TreeKeys({ up, only, narrow }: { up: () => boolean; only: () => boolean; narrow: () => boolean }) {
  useKeyAction('tree-up', up);
  useKeyAction('tree-only', only);
  useKeyAction('tree-filter', narrow);
  return null;
}

/** What lies over the page and was opened from where the keyboard was: a list of acts, a dialog. The keyboard in it is still where it came from. */
const OVER_THE_PAGE = '[role="dialog"], [role="alertdialog"], [role="menu"]';

const NO_NEEDS: SectionNeeds = { unread: false, waiting: 0, mentions: 0 };
const NO_GROUPS: readonly GroupChat[] = [];

interface Shortcut {
  place: PlaceRef;
  where: RowPlaceValue | null;
}

/** The address of a document's page and of a thread's, in the address of the screen in view. */
const DOCUMENT_ADDRESS = /^\/doc\/([^/]+)\/?$/;
const THREAD_ADDRESS = /^\/thread\/([^/]+)\/?$/;

/** Does the acts one after another, a few at a time, and says whether every one of them was done. */
async function allDone(acts: readonly (() => Promise<unknown>)[]): Promise<boolean> {
  let next = 0;
  let failed = false;
  const worker = async () => {
    for (let act = acts[next]; act !== undefined; act = acts[next]) {
      next += 1;
      try {
        await act();
      } catch {
        failed = true;
      }
    }
  };
  await Promise.all(Array.from({ length: Math.min(READ_AT_ONCE, acts.length) }, worker));
  return !failed;
}

/** Org actions belong to Navigation's existing title row, without a second org heading. */
export function NavigationActions({ entries, label }: Partial<ViewMenuProps> = {}) {
  const selected = useNavigationOrg();
  const orgs = useQuery({ ...orgsQuery, enabled: false }).data;
  const current = orgs?.find((org) => org.id === selected);
  const layoutEntries = entries?.map((entry): MenuEntry => ({ ...entry, id: `view:${entry.id}` }));
  if (current === undefined) return entries === undefined || label === undefined ? null : <Menu label={label} entries={entries} align="end" trigger={<IconButton action="actions" size="sm" label={label} icon={<Ellipsis aria-hidden />} />} />;
  return <NodeMenuButton place={placeOfNode(current)} node={current} extraEntries={layoutEntries} menuLabel={label} />;
}

export function Sidebar() {
  const orgId = useNavigationOrg();
  const orgs = useQuery({ ...orgsQuery, enabled: false }).data ?? [];
  const person = useQuery({ ...meQuery, enabled: false }).data;
  if (orgId === null) return <div className="flex h-(--control-md) items-stretch"><OrgSwitcher orgs={railOrgs(orgs, person)} current={null} /></div>;
  return <NavigationBody orgId={orgId} />;
}

function NavigationBody({ orgId }: { orgId: string | null }) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const openPath = useOpenPath();
  const userId = useStore(client.session, (state) => state.userId);
  const locale = useAppearance((state) => state.locale);
  const orgs = useQuery(orgsQuery);
  const person = useQuery(meQuery).data;
  const ownOrgs = railOrgs(orgs.data ?? [], person);
  const selected = useSelectedOrg((state) => state.orgId);
  const heading = useCurrentHeading();
  const preferences = useSidebarPreferences();
  const participationCopy = useParticipationCopy();
  const shape = useShape();
  const { show } = preferences;
  const shown = useViewShown();
  const [arrangedAs, chooseArrangement] = useViewChoice<string>('navigation', 'arrangement', 'tree', orgId ?? undefined);
  const [orderedBy, chooseRecentOrder] = useViewChoice<string>('navigation', 'recentOrder', 'opened', orgId ?? undefined);
  const arrangement: NavigationArrangement = arrangedAs === 'recent' ? 'recent' : 'tree';
  const recentOrder: RecentOrder = orderedBy === 'active' ? 'active' : 'opened';

  const current = orgs.data?.find((org) => org.id === orgId) ?? null;
  // The order of the tree: the reader's own choice in this org, else the org's.
  const treeOrder = useTreeOrder(orgId, shown);
  const sortedBy = treeOrder.order;
  // A row dropped beside another of its parent makes the reader's order their own, from the order they saw.
  const arrange = useCallback(
    (parentId: string, siblings: readonly string[], moved: string, before: string | null) => {
      const own = treeOrder.order.by === 'custom' ? (treeOrder.order.custom ?? {}) : {};
      treeOrder.choose.mutate(
        { by: 'custom', custom: { ...own, [parentId]: arranged(siblings, moved, before) } },
        { onError: () => toast({ tone: 'danger', title: t('navigation.order.notSaved') }) },
      );
    },
    [treeOrder.order, treeOrder.choose, t],
  );
  useEffect(() => {
    if (orgId !== null && orgId !== selected) selectOrg(userId, orgId);
  }, [orgId, selected, userId]);

  useOrgTreeFeed(shown ? orgId : null);
  useAttentionFeed(shown ? orgId : null);
  const tree = useOrgTree(orgId);
  const attention = useAttention(orgId);
  const currentId = heading?.nodeId ?? null;
  const currentDoc = useRouterState({ select: (state) => DOCUMENT_ADDRESS.exec(state.location.pathname)?.[1] ?? null });
  // The thread in view by its address: its screen says what it is only once it has read it, and a key pressed before that still starts from it.
  const currentThread = useRouterState({ select: (state) => THREAD_ADDRESS.exec(state.location.pathname)?.[1] ?? null });
  // Whether the screen in view shows the chat of its place: that chat's row is then the one in view.
  const chatInView = useRouterState({ select: (state) => (state.location.search as { panel?: unknown }).panel === 'chat' });
  const showArchived = useShowArchived(orgId);
  const unreadChats = useUnreadChats(orgId, shown && show.chats);

  // The reader's standing in chats, as their own feed keeps it, and the org's group chats they are in.
  const chatStates = useChatStates((state) => state.states);
  const chatLatest = useChatStates((state) => state.latest);
  const chats = useMemo<ChatStanding>(() => ({ states: chatStates, latest: chatLatest }), [chatStates, chatLatest]);
  const mine = useInfiniteQuery({ ...orgChatsQuery(orgId ?? '', { scope: 'mine', of: null }), enabled: shown && orgId !== null && show.chats });
  const groups = useMemo<readonly GroupChat[]>(
    () => (show.chats ? (mine.data?.pages.flatMap((page) => page.items).filter((chat) => chat.kind === 'group') ?? NO_GROUPS) : NO_GROUPS),
    [show.chats, mine.data],
  );

  // ---------------------------------------------------------------- which rows are open

  // What the device remembers, or orgs and projects the first time.
  const [opened, setOpened] = useState<{ orgId: string; ids: ReadonlySet<string> } | null>(null);
  const keptOpen = useMemo(() => (orgId === null ? null : readOpen(userId, orgId)), [orgId, userId]);
  const expanded = useMemo<ReadonlySet<string>>(() => {
    if (opened !== null && opened.orgId === orgId) return opened.ids;
    return keptOpen ?? new Set(tree.loaded ? firstExpanded(tree) : []);
  }, [opened, orgId, keptOpen, tree]);
  const setExpanded = useCallback(
    (next: ReadonlySet<string>) => {
      if (orgId === null) return;
      // Record only deliberate changes; initial path revelation never overwrites a chosen fold.
      for (const id of new Set([...expanded, ...next])) {
        if (expanded.has(id) !== next.has(id)) writeViewChoice(userId, 'navigation', 'treeExpanded', next.has(id), `${orgId}:${id}`);
      }
      setOpened({ orgId, ids: next });
    },
    [orgId, userId, expanded],
  );
  // What the reader opened and closed in this page is what the device opens next time.
  useEffect(() => {
    if (opened !== null) writeOpen(userId, opened.orgId, opened.ids);
  }, [opened, userId]);
  const onExpandedChange = useCallback(
    (id: string, open: boolean) => {
      const next = new Set(expanded);
      if (open) next.add(id);
      else next.delete(id);
      setExpanded(next);
    },
    [expanded, setExpanded],
  );

  // The screen in view where it is of something in this tree. The way to it, itself last: none of it is hidden as done.
  const here = currentId !== null && tree.nodes.has(currentId) ? currentId : null;
  const way = useMemo(() => (currentId === null || !tree.nodes.has(currentId) ? [] : pathTo(tree, currentId).map((node) => node.id)), [currentId, tree]);
  const wayKey = way.join(',');
  const keep = useMemo(() => new Set(wayKey === '' ? [] : wayKey.split(',')), [wayKey]);
  // The place a document or a chat in view stands beside is opened too, so that its own row is in view.
  const besideInView = (currentDoc !== null && show.documents) || (chatInView && show.chats);

  // An unchosen way is revealed on arrival; a deliberate fold waits for the person to open it.
  const [revealed, setRevealed] = useState<string | null>(null);
  const arrived = orgId === null || here === null || !tree.loaded ? null : `${orgId}:${here}:${besideInView ? 'beside' : ''}`;
  if (arrived !== null && arrived !== revealed && orgId !== null && here !== null) {
    setRevealed(arrived);
    const next = wayOpened(tree, expanded, here, besideInView);
    for (const id of next) {
      if (!expanded.has(id) && !readViewChoice<boolean>(userId, 'navigation', 'treeExpanded', true, `${orgId}:${id}`)) next.delete(id);
    }
    if (next.size !== expanded.size) setOpened({ orgId, ids: next });
  }

  // ---------------------------------------------------------------- where the tree begins

  // What the device remembers for this org, until the reader moves it in this page.
  const [begun, setBegun] = useState<{ orgId: string; nodeId: string | null } | null>(null);
  const keptBeginning = useMemo(() => (orgId === null ? null : readBeginning(userId, orgId)), [orgId, userId]);
  const wanted = begun !== null && begun.orgId === orgId ? begun.nodeId : keptBeginning;
  const beginning = useMemo(
    () => (orgId === null || !tree.loaded ? null : beginningOf(tree, orgId, wanted, showArchived.value)),
    [orgId, tree, wanted, showArchived.value],
  );
  const rootId = beginning ?? orgId;
  // Whether the reader moved the beginning in this page: only then is the tree seen to move.
  const moved = begun !== null;
  const setFolded = preferences.setFolded;
  const projectsFolded = preferences.isFolded('projects');
  const begin = useCallback(
    (nodeId: string | null) => {
      if (orgId === null) return;
      setBegun({ orgId, nodeId });
      // A tree that is asked to show something is shown.
      if (projectsFolded) setFolded('projects', false);
    },
    [orgId, projectsFolded, setFolded],
  );
  // A place that is gone narrows nothing, and does not narrow the tree again by itself if it comes back.
  if (orgId !== null && tree.loaded && wanted !== null && beginning === null) setBegun({ orgId, nodeId: null });
  // Where the reader put the beginning in this page is where the device begins next time.
  useEffect(() => {
    if (begun !== null) writeBeginning(userId, begun.orgId, begun.nodeId);
  }, [begun, userId]);

  // The rows the keyboard was on are gone when the beginning moves: the tree's first row takes it, so that the arrows go on from there.
  const bar = useRef<HTMLElement>(null);
  useEffect(() => {
    if (!moved) return;
    const frame = requestAnimationFrame(() => {
      const active = document.activeElement;
      if (active === null || active === document.body) bar.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]')?.focus();
    });
    return () => cancelAnimationFrame(frame);
  }, [rootId, moved]);

  const beginningWay = useMemo(() => (beginning === null ? [] : placesTo(tree, beginning)), [beginning, tree]);
  const up = useCallback(() => {
    if (orgId === null || beginning === null) return;
    begin(placeAbove(tree, orgId, beginning));
  }, [orgId, beginning, tree, begin]);

  // The thing in view, while it lies outside the place the tree begins at: the first row says where it lies.
  // The root org lies nowhere in its own tree, narrowed or not, and has no such row.
  const outside = useMemo(() => {
    if (beginning === null || currentId === null) return null;
    const node = tree.nodes.get(currentId);
    if (node === undefined || node.deleted || node.parent_id === null || liesIn(tree, currentId, beginning)) return null;
    const above = placesTo(tree, currentId).slice(0, -1);
    return { node, where: above.map((place) => place.name).join(i18n.t('navigation.pathJoin')), place: above.at(-1)?.name ?? '' };
  }, [beginning, currentId, tree]);
  const showThere = useCallback(() => {
    if (orgId === null || currentId === null) return;
    // The thing's own project is where it is shown: the tree begins there, and the way to the thing is open.
    begin(projectOf(tree, orgId, currentId));
    setExpanded(wayOpened(tree, expanded, currentId, besideInView));
  }, [orgId, currentId, tree, begin, setExpanded, expanded, besideInView]);

  // ---------------------------------------------------------------- what the tree shows

  // What the reader typed to narrow the tree by a name.
  const [filter, setFilter] = useState('');
  const filterField = useRef<HTMLInputElement>(null);
  // Once the rows are drawn again, the tree's own row takes the keyboard.
  const toFirstRow = () => requestAnimationFrame(() => bar.current?.querySelector<HTMLElement>('[role="treeitem"][tabindex="0"]')?.focus());

  // The documents beside the places that are open, asked for only where documents stand in the tree.
  const showsDocs = shown && arrangement === 'tree' && show.documents && !projectsFolded;
  const docs = useTreeDocsOf(
    useMemo(() => (showsDocs && rootId !== null ? placesWithDocs(tree, rootId, expanded, show.threads) : ''), [showsDocs, tree, rootId, expanded, show.threads]),
  );
  const view = useMemo<TreeView>(
    () => ({
      showArchived: showArchived.value,
        hideDone: preferences.hideDone,
        participating: preferences.participating,
      keep,
      attention,
      docs: show.documents ? docs : null,
      threads: show.threads,
      chats,
      chatRows: show.chats,
      // The current place's team chat has one home, the Team container.
      teamChat: false,
      groups,
      outside,
      filter,
      order: sortedBy,
    }),
    [showArchived.value, preferences.hideDone, preferences.participating, keep, attention, show.documents, show.threads, show.chats, docs, chats, groups, outside, filter, sortedBy],
  );

  const beginNode = beginning === null ? undefined : tree.nodes.get(beginning);
  const hasRows =
    rootId !== null &&
    (outside !== null ||
      (show.documents && (tree.nodes.get(rootId)?.docs ?? 0) > 0) ||
      (show.chats && (groups.length > 0 || (tree.nodes.get(rootId)?.chat.seq ?? 0) > 0)) ||
      [...tree.nodes.values()].some((node) => node.parent_id === rootId && !node.deleted && (showArchived.value || !node.archived) && (show.threads || node.kind !== 'thread')));
  const rootPlace = current === null ? null : placeOfNode(current);
  // What "+" makes things in: the place the tree begins at.
  const makeIn = beginNode === undefined ? rootPlace : placeOfRow(beginNode, beginNode.kind === 'project' && tree.nodes.get(beginNode.parent_id ?? '')?.kind === 'project');

  // ---------------------------------------------------------------- starred and recent

  const favorites = useFavorites();
  const starred = useMemo<Shortcut[]>(
    () =>
      favorites
        .filter((favorite) => (favorite.path[0]?.id ?? favorite.node.id) === orgId && favorite.node.name !== null)
        .map((favorite) => ({
          place: {
            id: favorite.node.id,
            kind: favorite.node.kind,
            name: favorite.node.name ?? '',
            parentId: favorite.path.at(-1)?.id ?? null,
            archived: false,
            status: tree.nodes.get(favorite.node.id)?.topic?.status ?? null,
            folder: favorite.node.kind === 'project' && favorite.path.at(-1)?.kind === 'project',
          },
          where: nearestPlace({ id: favorite.node.id, path: favorite.path, place_label: null }),
        })),
    [favorites, orgId, tree.nodes],
  );

  const names = useQuery({ ...recentsOfOrgQuery(orgId ?? ''), enabled: shown && arrangement === 'tree' && orgId !== null && !preferences.isFolded('recent') });
  const seen = useSeenHere();
  const recent = useMemo<(Shortcut | { doc: DocPlace; where: RowPlaceValue | null })[]>(() => {
    const found: (Shortcut | { doc: DocPlace; where: RowPlaceValue | null })[] = [];
    if (names.isSuccess) {
      for (const hit of names.data) {
        if (hit.kind === 'document') {
          const beside = hit.path.at(-1);
          if (hit.id === currentDoc || beside === undefined) continue;
          found.push({ doc: { id: hit.id, title: hit.name, node: beside, archived: hit.archived, owner: hit.owner }, where: nearestPlace({ id: hit.id, path: hit.path, place_label: null }) });
          continue;
        }
        if (hit.id === orgId || (hit.id === currentId && currentDoc === null)) continue;
        found.push({
          place: {
            id: hit.id,
            kind: hit.kind as NodeKind,
            name: hit.name,
            parentId: hit.path.at(-2)?.id ?? null,
            archived: hit.archived,
            status: tree.nodes.get(hit.id)?.topic?.status ?? null,
            folder: hit.kind === 'project' && hit.path.at(-2)?.kind === 'project',
          },
          where: nearestPlace({ id: hit.id, path: hit.path, place_label: null }),
        });
      }
    } else if (names.isError) {
      // The system does not answer the question: what this device saw itself is listed instead.
      for (const id of seen) {
        const node = tree.nodes.get(id);
        if (node === undefined || node.deleted || node.archived || node.id === orgId || node.id === currentId) continue;
        const folder = node.kind === 'project' && tree.nodes.get(node.parent_id ?? '')?.kind === 'project';
        const parents = pathTo(tree, node.id).slice(0, -1);
        const parent = parents.at(-1);
        found.push({
          place: { id: node.id, kind: node.kind, name: node.name, parentId: node.parent_id, archived: false, status: node.topic?.status ?? null, folder },
          where: parent === undefined ? null : { name: parent.name, path: parents.map((part) => part.name).join(i18n.t('navigation.pathJoin')) },
        });
      }
    }
    return found.slice(0, RECENT_ROWS);
  }, [names.isSuccess, names.isError, names.data, seen, tree, orgId, currentId, currentDoc]);

  // What lies under what is worked out once for a tree, and only when something asks.
  const below = useMemo(() => {
    let held: Below | null = null;
    return () => (held ??= belowOf(tree.nodes));
  }, [tree.nodes]);
  const needsIn = useCallback(
    (ids: readonly string[]) => needsOf(tree, ids, attention, showArchived.value, chats, below()),
    [tree, attention, showArchived.value, chats, below],
  );
  const starredFolded = preferences.isFolded('starred');
  const recentFolded = preferences.isFolded('recent');
  const starredNeeds = useMemo(() => (starredFolded ? needsIn(starred.map((entry) => entry.place.id)) : NO_NEEDS), [starredFolded, needsIn, starred]);
  const recentNeeds = useMemo(() => (recentFolded ? needsIn(recent.flatMap((entry) => ('place' in entry ? [entry.place.id] : []))) : NO_NEEDS), [recentFolded, needsIn, recent]);
  const projectsNeeds = useMemo(() => {
    if (!projectsFolded || rootId === null) return NO_NEEDS;
    // The team's chat is said by its own row above while it stands there, and not a second time by the heading.
    const own = rootId !== orgId;
    return needsIn(own ? [rootId] : (below().get(rootId) ?? []).map((node) => node.id));
  }, [projectsFolded, rootId, orgId, needsIn, below]);

  // ---------------------------------------------------------------- what a row's menu asks of the tree

  const markRead = useCallback(
    (nodeId: string) => {
      const pending: TreeNode[] = [];
      const first = tree.nodes.get(nodeId);
      if (first !== undefined && !first.deleted) pending.push(first);
      const acts: (() => Promise<unknown>)[] = [];
      for (let node = pending.pop(); node !== undefined; node = pending.pop()) {
        if (node.thread !== null && isUnread(node, tree.nodes, attention)) {
          const { id } = node;
          const seq = node.thread.activity_seq;
          // Shown at once: the server's answer says how far they have read, which may be further.
          attentionWriter.read(id, seq);
          acts.push(() => markThreadRead(id, seq).then((read) => attentionWriter.read(id, read)));
        }
        const chat = chatStanding(node.chat, chats);
        if (chat.unread || chat.mentions > 0) readChat(node.chat.id, Math.max(node.chat.seq, chats.latest.get(node.chat.id)?.seq ?? 0));
        pending.push(...(below().get(node.id) ?? []));
      }
      void allDone(acts).then((done) => {
        if (done) return;
        // What the server holds stands: the positions are read again, and the person is told.
        reloadAttention();
        toast({ tone: 'danger', title: i18n.t('navigation.notMarkedRead') });
      });
    },
    [tree.nodes, attention, chats, below],
  );
  useEffect(() => {
    if (orgId === null || !tree.loaded) return;
    useTreeActs.setState({
      acts: {
        canShowOnly: (nodeId) => beginningOf(tree, orgId, nodeId, showArchived.value) !== null && nodeId !== beginning,
        showOnly: begin,
        collapseOthers: (nodeId) => setExpanded(othersFolded(tree, expanded, nodeId)),
        hasUnread: (nodeId) => {
          const needs = needsIn([nodeId]);
          return needs.unread || needs.mentions > 0;
        },
        markRead,
      },
    });
    return () => useTreeActs.setState({ acts: null });
  }, [orgId, tree, showArchived.value, beginning, begin, setExpanded, expanded, needsIn, markRead]);

  // ---------------------------------------------------------------- the keys

  const walk = useRef<(step: 1 | -1, unread: boolean) => boolean>(() => false);
  const only = useRef<() => boolean>(() => false);
  const narrow = useRef<() => boolean>(() => false);
  const upKey = useRef<() => boolean>(() => false);
  useEffect(() => {
    // The keys of the tree belong to the bar: pressed while the keyboard is elsewhere they are not the bar's to take.
    const inBar = () => bar.current?.contains(document.activeElement) === true;
    narrow.current = () => {
      if (!inBar()) return false;
      if (projectsFolded) setFolded('projects', false);
      requestAnimationFrame(() => filterField.current?.focus());
      return true;
    };
    upKey.current = () => {
      if (!inBar()) return false;
      // Where there is no place above, the key says so and does nothing.
      if (orgId === null || beginning === null) toast({ tone: 'info', title: i18n.t('navigation.begins.atTop') });
      else begin(placeAbove(tree, orgId, beginning));
      return true;
    };
    walk.current = (step, unread) => {
      if (rootId === null || !tree.loaded) return false;
      // Where the keys start from: the chat in view where chats are walked, else the thread, else the place.
      const withChats = unread && show.chats;
      const from = withChats && chatInView && currentId !== null ? chatRowId(currentId) : (currentThread ?? currentId);
      const stops = stopsInOrder(tree, rootId, locale, view, undefined, withChats);
      const candidates = unread ? stops.filter((stop) => stop.unread || stop.id === from) : stops;
      if (candidates.length === 0) return false;
      const at = candidates.findIndex((stop) => stop.id === from);
      // From a screen that is none of them the first key goes to the first, or to the last going up.
      const next = at === -1 ? (step === 1 ? 0 : candidates.length - 1) : at + step;
      // The unread ones are walked round and round; the threads of the tree end where the tree ends.
      const target = unread ? candidates[(next + candidates.length) % candidates.length] : candidates[next];
      if (target === undefined || target.id === from) return false;
      if (target.kind === 'chat') openPath(chatPath(target.node));
      else void navigate({ to: '/thread/$threadId', params: { threadId: target.id } });
      return true;
    };
    only.current = () => {
      if (orgId === null || !tree.loaded || !inBar()) return false;
      // The row the keyboard is on. A tree begins at a place: on the row of anything else the key says what it takes.
      const row = document.activeElement?.closest<HTMLElement>('[data-tree-row]')?.dataset.treeRow;
      if (row === undefined) return false;
      if (beginningOf(tree, orgId, row, showArchived.value) === null) toast({ tone: 'info', title: i18n.t('navigation.begins.cannot') });
      else begin(row);
      return true;
    };
  }, [tree, orgId, rootId, locale, view, show.chats, chatInView, currentId, currentThread, navigate, openPath, showArchived.value, beginning, begin, projectsFolded, setFolded]);
  const previousThread = useCallback(() => walk.current(-1, false), []);
  const nextThread = useCallback(() => walk.current(1, false), []);
  const previousUnread = useCallback(() => walk.current(-1, true), []);
  const nextUnread = useCallback(() => walk.current(1, true), []);
  const showOnlyRow = useCallback(() => only.current(), []);
  const filterKey = useCallback(() => narrow.current(), []);
  const stepUp = useCallback(() => upKey.current(), []);
  // The four are one line of the list of keys, which shows the first and the last of them: the plain pair, with Shift said in words.
  useKeyAction('previous-thread', previousThread);
  useKeyAction('previous-unread', previousUnread);
  useKeyAction('next-unread', nextUnread);
  useKeyAction('next-thread', nextThread);

  // Whether the keyboard is in the bar, or in something that was opened from it and lies over the page.
  const [keyboardHere, setKeyboardHere] = useState(false);
  useEffect(() => {
    const onFocusIn = (event: FocusEvent) => {
      const target = event.target instanceof Element ? event.target : null;
      if (target === null) return;
      if (bar.current?.contains(target) === true) setKeyboardHere(true);
      else if (target.closest(OVER_THE_PAGE) === null) setKeyboardHere(false);
    };
    // The keyboard that leaves for nothing at all, as after a press on the page's ground, has left the bar.
    const onFocusOut = (event: FocusEvent) => {
      if (event.relatedTarget === null && document.hasFocus()) setKeyboardHere(false);
    };
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
    };
  }, []);

  // ---------------------------------------------------------------- the tree's own menu

  const nearer = orgId === null ? null : whereIAm(tree, orgId, currentId, beginning, showArchived.value);
  const openToHere = () => {
    if (here === null) return;
    setExpanded(wayOpened(tree, expanded, here, besideInView));
    if (projectsFolded) setFolded('projects', false);
    // Once the rows are drawn, the one in view is brought into sight.
    requestAnimationFrame(() => document.querySelector(`[data-tree-row="${CSS.escape(here)}"]`)?.scrollIntoView({ block: 'nearest' }));
  };
  const [revealing, setRevealing] = useState<{ id: string; nodeId: string; documentId?: string } | null>(null);
  const reveal = useCallback((row: EverythingRow) => {
    const beside = row.kind === 'document' || row.kind === 'chat';
    const groupChat = row.kind === 'chat' && row.path.length === 0;
    const id = beside ? row.path.at(-1)?.id ?? row.parent_id ?? undefined : row.id;
    if (id === undefined || orgId === null || !tree.nodes.has(id)) return;
    chooseArrangement('tree');
    if (beginning !== null && !liesIn(tree, id, beginning)) begin(projectOf(tree, orgId, id));
    const next = wayOpened(tree, expanded, id, beside);
    if (groupChat) next.add(GROUPS_ROW);
    setExpanded(next);
    if (projectsFolded) setFolded('projects', false);
    setRevealing({ id: row.kind === 'document' ? docRowId(row.id) : groupChat ? groupRowId(row.id) : row.kind === 'chat' ? chatRowId(id) : id, nodeId: id,
      ...(row.kind === 'document' ? { documentId: row.id } : {}) });
  }, [orgId, tree, chooseArrangement, beginning, begin, expanded, setExpanded, projectsFolded, setFolded]);
  const requested = useTreeReveal((state) => state.row);
  useEffect(() => {
    if (requested === null || (requested.path[0]?.id ?? requested.parent_id) !== orgId || !tree.loaded) return;
    let live = true;
    queueMicrotask(() => {
      if (!live || useTreeReveal.getState().row !== requested) return;
      reveal(requested);
      useTreeReveal.setState({ row: null });
    });
    return () => { live = false; };
  }, [requested, orgId, tree.loaded, reveal]);
  useEffect(() => {
    if (revealing === null || arrangement !== 'tree') return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const frame = requestAnimationFrame(() => {
      const row = bar.current?.querySelector<HTMLElement>(`[data-tree-row="${CSS.escape(revealing.id)}"]`);
      if (row === undefined || row === null) {
        const page = docs.get(revealing.nodeId);
        if (revealing.documentId !== undefined && page?.state === 'ready' && !page.loadingMore) {
          if (page.more) loadMoreDocs(revealing.nodeId);
          else setRevealing(null);
        }
        return;
      }
      row.scrollIntoView({ block: 'nearest' }); row.focus(); row.dataset.navigationRevealed = 'true';
      timer = setTimeout(() => { row.removeAttribute('data-navigation-revealed'); setRevealing(null); }, REVEAL_MS);
    });
    return () => { cancelAnimationFrame(frame); clearTimeout(timer); };
  }, [revealing, arrangement, docs]);
  // Why the tree cannot be narrowed to where the reader is, where it cannot.
  const noNearer = nearer !== null ? null : here === null ? 'navigation.begins.notInTree' : beginning !== null && liesIn(tree, here, beginning) ? 'navigation.begins.already' : 'navigation.begins.notInside';
  // The same acts in the finder, which also finds them by the words other tools use for them.
  const words = useCallback((t2: TFunction) => t2('navigation.begins.words').split(/\s+/), []);
  const upKeys = useActionKeys('tree-up');
  const commands = useMemo<Command[]>(
    () => [
      {
        id: 'tree.where-i-am',
        kind: 'view',
        title: (t2) => t2('navigation.begins.whereIAm'),
        keywords: words,
        icon: Focus,
        run: () => begin(nearer),
        available: () => (noNearer === null ? true : { disabled: i18n.t(noNearer) }),
      },
      { id: 'tree.up', kind: 'view', title: (t2) => t2('navigation.begins.up'), keywords: words, icon: ArrowUp, shortcut: upKeys, run: up, available: () => beginning !== null },
      { id: 'tree.whole-org', kind: 'view', title: (t2) => t2('navigation.begins.whole'), keywords: words, icon: ListTree, run: () => begin(null), available: () => beginning !== null },
    ],
    [words, begin, nearer, noNearer, beginning, up, upKeys],
  );
  useRegisterCommands(commands);

  const viewEntries: MenuEntry[] = [
    { id: 'open-everything', label: t('navigation.openEverything'), icon: <ListTree aria-hidden />, onSelect: () => {
      const address = new URLSearchParams({ parent: rootId ?? '', org: orgId ?? '', arrangement, order: recentOrder,
        kinds: navigationFilter(recentOrder, filter, show, showArchived.value, preferences.hideDone, preferences.participating === true).kinds.join(','),
        showArchived: String(showArchived.value), hideDone: String(preferences.hideDone), navigation: 'true', participating: String(preferences.participating === true) });
      if (filter.trim() !== '') address.set('q', filter.trim());
      openPath(`/everything?${address}`);
    } },
    { type: 'separator', id: 'separator-everything' },
    { type: 'label', id: 'show', label: t('navigation.view.show') },
    { type: 'check', id: 'threads', ...choiceOption(t, 'navigationShow.threads'), checked: show.threads, onCheckedChange: (on) => preferences.setShown('threads', on) },
    { type: 'check', id: 'documents', ...choiceOption(t, 'navigationShow.documents'), checked: show.documents, onCheckedChange: (on) => preferences.setShown('documents', on) },
    { type: 'check', id: 'chats', ...choiceOption(t, 'navigationShow.chats'), checked: show.chats, onCheckedChange: (on) => preferences.setShown('chats', on) },
    { type: 'separator', id: 'separator-hide' },
    { type: 'check', id: 'hide-done', ...choiceOption(t, 'navigationShow.hide_done'), checked: preferences.hideDone, onCheckedChange: preferences.setHideDone },
    { type: 'check', id: 'participating', ...participationCopy, checked: preferences.participating === true, onCheckedChange: preferences.setParticipating },
    { type: 'check', id: 'show-archived', ...choiceOption(t, 'navigationShow.archived'), description: t('places.menu.showArchived'), checked: showArchived.value, onCheckedChange: showArchived.toggle },
    { type: 'separator', id: 'separator-begins' },
    {
      id: 'where-i-am',
      label: t('navigation.begins.whereIAm'),
      icon: <Focus aria-hidden />,
      onSelect: () => begin(nearer),
      ...(noNearer === null ? {} : { disabled: t(noNearer) }),
    },
    ...(beginning === null ? [] : [{ id: 'whole-org', label: t('navigation.begins.whole'), icon: <ListTree aria-hidden />, onSelect: () => begin(null) } satisfies MenuEntry]),
    { type: 'separator', id: 'separator-open' },
    { id: 'open-to-here', label: t('navigation.view.openToHere'), icon: <Locate aria-hidden />, onSelect: openToHere, ...(here === null ? { disabled: t('navigation.begins.notInTree') } : {}) },
    { id: 'collapse-all', label: t('navigation.view.collapseAll'), icon: <ChevronsDownUp aria-hidden />, onSelect: () => setExpanded(new Set()) },
    { type: 'separator', id: 'separator-order' },
    {
      type: 'radio',
      id: 'order',
      label: t('navigation.order.label'),
      value: treeOrder.choice,
      onValueChange: (value) => {
        const choice = value as OrderChoice;
        // One's own order keeps what was arranged before; the others need nothing kept.
        const own = treeOrder.choice === 'custom' ? treeOrder.order : { by: 'custom' as const };
        treeOrder.choose.mutate(choice === 'default' ? null : choice === 'custom' ? own : { by: choice }, {
          onError: () => toast({ tone: 'danger', title: t('navigation.order.notSaved') }),
        });
      },
      options: [
        { value: 'default', ...choiceOption(t, 'navigationOrder.default', { org: current?.name ?? '' }) },
        { value: 'custom', ...choiceOption(t, 'navigationOrder.custom') },
        { value: 'newest', ...choiceOption(t, 'navigationOrder.newest') },
        { value: 'changed', ...choiceOption(t, 'navigationOrder.changed') },
        { value: 'name', ...choiceOption(t, 'navigationOrder.name') },
      ],
    },
    // Whoever owns or administers the org makes the order they see the one everybody follows by default.
    ...(current !== null && (current.access.administers || current.access.owns) && treeOrder.choice !== 'default'
      ? [{ id: 'save-order', label: t('navigation.order.saveForEveryone', { org: current.name }), onSelect: () =>
            treeOrder.saveForEveryone.mutate(undefined, {
              onSuccess: () => toast({ tone: 'success', title: t('navigation.order.saved', { org: current.name }) }),
              onError: () => toast({ tone: 'danger', title: t('navigation.order.notSaved') }),
            }) } satisfies MenuEntry]
      : []),
  ];

  const currentRow = currentDoc !== null ? docRowId(currentDoc) : chatInView && currentId !== null ? chatRowId(currentId) : null;
  const showTeamChat = current !== null && current.org_kind !== 'personal' && holds(current.access, 'chat.view');

  return (
    <nav ref={bar} aria-label={t('views.navigation.title')} className="flex h-full min-h-0 flex-col bg-bar-surface">
      {keyboardHere && <TreeKeys up={stepUp} only={showOnlyRow} narrow={filterKey} />}
      {shape === 'small' && <div className="flex h-(--control-md) shrink-0 items-stretch border-b border-border">
        <OrgSwitcher orgs={ownOrgs} current={current} />
      </div>}
      <div data-bar-scroll className="flex min-h-0 flex-1 flex-col overflow-y-auto">
        {current !== null && (
          <ul className="flex flex-col py-1">
            {showTeamChat && <li className="flex min-w-0 flex-col"><TeamChatShortcut nodeId={current.id} chatId={current.chat_id} seq={tree.nodes.get(current.id)?.chat.seq ?? 0} className={ORG_ROW} /></li>}
            <li className="flex min-w-0 flex-col">
            <Link to="/org/$slug/messages" params={{ slug: current.slug }} className={ORG_ROW}>
              <OrgRow icon={<MessagesSquare aria-hidden />} marks={<Badge tone="accent" count={unreadChats} label={t('navigation.counts.chats', { count: unreadChats })} />}>
                {t('screens.messages.title')}
              </OrgRow>
            </Link>
            </li>
          </ul>
        )}

        {starred.length > 0 && (
          <Section title={t('navigation.starred')} folded={starredFolded} onFoldedChange={(folded) => setFolded('starred', folded)} needs={starredNeeds}>
            {starred.map(({ place, where }) => (
              <ShortcutRow key={place.id} place={place} color={tree.nodes.get(place.id)?.color ?? null} where={where} needs={needsIn([place.id])} />
            ))}
          </Section>
        )}

        {arrangement === 'tree' && <Section title={t('navigation.recent')} folded={recentFolded} onFoldedChange={(folded) => setFolded('recent', folded)} needs={recentNeeds}
          action={<button type="button" className="focus-inset rounded-md px-2 text-sm text-text-muted hover:text-text" onClick={() => { chooseArrangement('recent'); setFolded('projects', false); }}>{t('navigation.recentAll')}</button>}>
          {recent.length === 0 ? (
            <Nothing>{t('navigation.noRecent')}</Nothing>
          ) : (
            recent.map((entry) =>
              'doc' in entry ? (
                <DocumentShortcut key={entry.doc.id} doc={entry.doc} where={entry.where} />
              ) : (
                <ShortcutRow key={entry.place.id} place={entry.place} color={tree.nodes.get(entry.place.id)?.color ?? null} where={entry.where} needs={needsIn([entry.place.id])} />
              ),
            )
          )}
        </Section>}

        <Section
          title={t('navigation.projects')}
          folded={projectsFolded}
          onFoldedChange={(folded) => setFolded('projects', folded)}
          needs={projectsNeeds}
          path={beginning === null ? undefined : <Beginning way={beginningWay} onBegin={begin} onUp={up} />}
          action={
              <span className="flex shrink-0 items-center">
                <ArrangementSwitch value={arrangement} onChange={(value) => { chooseArrangement(value); if (projectsFolded) setFolded('projects', false); }} />
                {makeIn !== null && current !== null && <NodeMakeButton place={makeIn} node={beginning === null ? current : undefined} />}
                <Menu
                  label={t('navigation.view.label')}
                  align="end"
                  entries={viewEntries}
                  trigger={<IconButton size="sm" label={t('navigation.view.label')} icon={<Ellipsis aria-hidden />} />}
                />
              </span>
          }
        >
          {(arrangement === 'recent' || hasRows || filter !== '') && <div className="flex min-w-0 flex-wrap items-center pr-2">
            <div className="min-w-32 flex-1"><TreeFilter short={arrangement === 'recent'} ref={filterField} value={filter} onChange={setFilter} onLeave={toFirstRow} onDown={toFirstRow} /></div>
            {arrangement === 'recent' && <RecentOrderSelect value={recentOrder} onChange={chooseRecentOrder} />}
          </div>}
          {arrangement === 'recent' && userId !== null && rootId !== null && orgId !== null ? <RecentList reader={userId} parent={rootId} orgId={orgId} tree={tree} current={currentDoc !== null ? `document:${currentDoc}` : currentId === null ? null : `${tree.nodes.get(currentId)?.kind ?? 'thread'}:${currentId}`}
            filter={navigationFilter(recentOrder, filter, show, showArchived.value, preferences.hideDone, preferences.participating === true, currentId ?? undefined)} needs={needsIn} onShowTree={reveal} /> : <>
          <div
            // The tree is seen to move when its reader moves its beginning, and is simply there otherwise.
            key={rootId ?? 'none'}
            className={cx(
              'flex flex-col transition-opacity duration-(--motion-base) ease-(--motion-ease) motion-reduce:transition-none',
              moved && 'starting:opacity-0',
              tree.error !== null && tree.loaded && 'opacity-60',
            )}
          >
            {orgs.isError ? (
              <ErrorState compact error={orgs.error} retrying={orgs.isRefetching} onRetry={() => void orgs.refetch()} />
            ) : orgId === null || rootId === null || (!tree.loaded && tree.error === null) ? (
              <SkeletonRows rows={6} />
            ) : !tree.loaded ? (
              <ErrorState compact error={tree.error} onRetry={() => reloadOrgTree(orgId)} />
            ) : hasRows ? (
              <NodeTree
                tree={tree}
                rootId={rootId}
                view={view}
                expanded={expanded}
                onExpandedChange={onExpandedChange}
                currentId={currentId}
                currentRow={currentRow}
                locale={locale}
                me={userId}
                orgSlug={current?.slug ?? null}
                onShowThere={showThere}
                onArrange={arrange}
                {...(filter.trim() === '' ? {} : { nothing: t('navigation.filter.none') })}
              />
            ) : (
              <EmptyState compact icon={<FolderOpen aria-hidden />} title={t(beginning === null ? 'navigation.noProjects' : 'navigation.begins.empty')} />
            )}
          </div>
          {tree.loaded && tree.error !== null && orgId !== null && <ErrorState compact error={tree.error} onRetry={() => reloadOrgTree(orgId)} />}
          </>}
        </Section>
      </div>
    </nav>
  );
}
