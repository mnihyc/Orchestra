/** The tree of the left bar as rows in order: what stands where, what each row carries of what it hides, and what is left out, worked out from the tree and from what its reader chose and has read.
 *
 * Brothers stand in the order of their kinds, then by name as the language sorts it; a place's
 * documents stand after what else it holds, and its chat after those. A topic that is done stands
 * where it stood. The tree begins at the root org or at the place its reader narrowed it to.
 *
 * One rule for every row that hides something, whether a place that is closed or a topic whose
 * threads are out of the tree: it is unread when anything in it that the reader follows is unread,
 * threads and chats alike; it carries a number only for what waits for the reader in person, the
 * decisions that wait and the times they are mentioned, and never a sum of unread things. A chat the
 * reader does not follow is quiet, counts for nothing above it, and still carries a mention.
 *
 * A reader who types a few letters narrows the tree to the places and threads that have them in their
 * name: each of those is shown with the way down to it open, and with what lies in it as its reader
 * left it; everything else is left out.
 *
 * The laying out is kept cheap for a tree of thousands: what stands under what is sorted again only
 * when a node moved, and a row whose item did not change keeps the item it had, so that it is not
 * drawn again.
 */
import type { DocumentRef } from '@/api/documents';
import { matches as matchesQuery } from '@/features/finder/ranking';
import { participatingTree } from '@/features/access/participation';
import { type Attention, isUnread, NO_ATTENTION } from '@/features/navigation/attention';
import type { DocsByNode } from '@/features/navigation/treeDocs';
import type { TreeOrder } from '@/features/navigation/treeOrder';
import type { Chat, ChatState, HiddenItem, TreeNode } from '@/generated/api-types';
import type { OrgTree } from '@/realtime/stores/tree';
import type { WorkState } from '@/ui/StateChip';
import { THREAD_URGENCY, isThreadUrgency, threadDisplayState } from '@/ui/ThreadState';
import type { TreeRow } from '@/ui/Tree';

/** What the small glyph of a place's chat says: that there is none to show, that the reader does not follow it, that they have read it, or that they have not. */
export type ChatMark = 'none' | 'quiet' | 'read' | 'unread';

/** What the tree needs of a group chat. */
export type GroupChat = Pick<Chat, 'id' | 'name' | 'last_message_seq' | 'my'>;

/**
 * What a row of the tree is of.
 *
 * Of a node: `folder` says that a project lies in a project. `urgent` is a thread's own state, the
 * most urgent state among a topic's threads, and for any other row that is closed the most urgent
 * state among everything it hides. `waiting` is how many decisions wait: in a thread, or in what a
 * row hides. `unread` says that a thread is unread, or that something a row hides is. `mentions` is
 * how often the reader is mentioned in the chats a row hides. `chat` is what the glyph of the node's
 * own chat says while that chat has no row of its own on screen, with `chatMentions` beside it. A
 * place that is closed hides its own chat with everything else: the row then says that it is unread
 * and carries its mentions, and the glyph is only the way to the chat.
 */
export type NavItem =
  | { type: 'node'; node: TreeNode; urgent: WorkState | null; folder: boolean; waiting: number; unread: boolean; mentions: number; chat: ChatMark; chatMentions: number }
  /** A document written beside a place. */
  | { type: 'doc'; doc: DocumentRef; nodeId: string }
  /** The line under a place's documents: that they are on their way, could not be read, or are more than were read. */
  | { type: 'docs'; nodeId: string; state: 'loading' | 'failed' | 'more' | 'loadingMore' }
  /** The chat of a place or a thread, as a row of its own. `quiet`: the reader does not follow it. */
  | { type: 'chat'; node: TreeNode; unread: boolean; mentions: number; quiet: boolean }
  /** The one row the org's group chats stand under: how many there are, and what they carry while it is closed. */
  | { type: 'groups'; count: number; unread: boolean; mentions: number }
  /** One group chat of the org. */
  | { type: 'group'; chat: GroupChat; unread: boolean; mentions: number; quiet: boolean }
  /** The first line of a tree that was narrowed to a place, while the thing in view lies outside it: what that thing is, the way to where it lies, and the place it lies in. */
  | { type: 'outside'; node: TreeNode; where: string; place: string }
  /** A private place the reader administers and may not view: its kind, name and first owner, and the one way to its permission page. */
  | { type: 'private'; row: HiddenItem };

const KIND_ORDER: Record<TreeNode['kind'], number> = { org: 0, project: 1, topic: 2, thread: 3 };

/** The id of the row of a document, which is no node. */
export const docRowId = (documentId: string) => `doc:${documentId}`;
/** The id of the line under the documents of a node. */
const docsRowId = (nodeId: string) => `docs:${nodeId}`;
/** The id of the row of a place's chat. */
export const chatRowId = (nodeId: string) => `chat:${nodeId}`;
/** The id of the row the org's group chats stand under, which opens and closes like a place. */
export const GROUPS_ROW = 'groups';
/** The id of the row of one group chat. */
export const groupRowId = (chatId: string) => `group:${chatId}`;
/** The id of the line that says the thing in view lies outside what the tree shows. */
export const OUTSIDE_ROW = 'outside';

/** The states of threads that a row shows for what it holds, most urgent first. */
const URGENCY = THREAD_URGENCY;

/** The reader's standing in chats, as their own feed keeps it. */
export interface ChatStanding {
  /** Their state in each chat they have one in. */
  states: ReadonlyMap<string, Pick<ChatState, 'effective_level' | 'last_read_seq' | 'mentions_unread' | 'hidden'>>;
  /** The newest message heard of in a chat since its place's row was read. */
  latest: ReadonlyMap<string, { seq: number }>;
}

export const NO_CHATS: ChatStanding = { states: new Map(), latest: new Map() };

/** What the reader chose to see, and what they have read. */
export interface TreeView {
  participating?: boolean;
  /** Whether archived things are shown. */
  showArchived?: boolean;
  /** Whether topics that are done are left out. */
  hideDone?: boolean;
  /** The nodes on the way to the screen in view, itself included: none of them is hidden. */
  keep?: ReadonlySet<string>;
  /** How far the reader has read, and what they follow. */
  attention?: Attention;
  /** The documents beside the places that are open, by place; `null` where documents do not stand in the tree. */
  docs?: DocsByNode | null;
  /** Whether threads stand in the tree; without them it is the map of places alone. Left out, they do. */
  threads?: boolean;
  /** The reader's standing in chats: what says that a chat is unread, whether or not chats have rows. */
  chats?: ChatStanding;
  /** Whether the chat of a place has a row of its own, after the place's documents. */
  chatRows?: boolean;
  /** Whether the root org's chat has its row though nothing was said in it yet: it is the way to the team's chat. */
  teamChat?: boolean;
  /** The org's group chats that the reader is in, where chats have rows: they stand under one row where the tree begins at the org itself. */
  groups?: readonly GroupChat[];
  /** What the reader typed to narrow the tree: only places and threads with it in their name are shown, each with the way to it. Empty or left out, the tree is whole. */
  filter?: string;
  /** The thing in view, the way to where it lies and the place it lies in, while it lies outside the place the tree begins at: the first row says so. */
  outside?: { node: TreeNode; where: string; place: string } | null;
  /** The order the reader's tree stands in; by name when left out. */
  order?: TreeOrder;
}

/** How a chat stands for its reader. */
interface ChatOf {
  /** Whether anybody said anything in it. */
  has: boolean;
  /** Whether the reader follows it: a chat they do not follow is quiet and counts for nothing above it. */
  followed: boolean;
  unread: boolean;
  /** How often they are mentioned in what they have not read: carried whether they follow it or not. */
  mentions: number;
}

const NO_CHAT: ChatOf = { has: false, followed: false, unread: false, mentions: 0 };

/** How a chat stands for its reader, from the newest message known of it and their state in it. `my` is their state as the chat itself said it, for a chat that came in a list. */
export function chatStanding(chat: { id: string; seq: number; my?: ChatState | null }, standing: ChatStanding): ChatOf {
  const newest = Math.max(chat.seq, standing.latest.get(chat.id)?.seq ?? 0);
  if (newest <= 0) return NO_CHAT;
  const state = standing.states.get(chat.id) ?? chat.my ?? null;
  if (state === null) return { has: true, followed: false, unread: false, mentions: 0 };
  const followed = state.effective_level !== 'off';
  return { has: true, followed, unread: followed && newest > state.last_read_seq, mentions: state.mentions_unread };
}

/**
 * How the nodes stood at the last laying out: which stand under which, in what order. A change that moves nothing (a thread that
 * changes its state, a topic that is marked done, one that is renamed to what sorts the same) is laid out from this and sorts nothing again.
 */
interface Shape {
  nodes: ReadonlyMap<string, TreeNode>;
  locale: string;
  showArchived: boolean;
  /** The order it was sorted in, compared by identity: the reader's choice is one object until they change it. */
  sortedBy: TreeOrder | undefined;
  /** For each node, the ids of what lies directly under it, in the order it stands. */
  order: ReadonlyMap<string, readonly string[]>;
}

/** What the last laying out was made from: the item each row last had, by the row's id, and how the nodes stood. */
export interface LayoutMemory {
  items: Map<string, NavItem>;
  shape: Shape | null;
  /** The documents of a place in the order they stand, by the list they were sorted from. */
  sorted: WeakMap<readonly DocumentRef[], { locale: string; docs: readonly DocumentRef[] }>;
}

export const newLayoutMemory = (): LayoutMemory => ({ items: new Map(), shape: null, sorted: new WeakMap() });

/** Whether a node stands where it stood: under the same node, of the same kind and name, and as much in view. */
const standsAsBefore = (one: TreeNode, other: TreeNode): boolean =>
  one.parent_id === other.parent_id && one.kind === other.kind && one.name === other.name && one.deleted === other.deleted && one.archived === other.archived;

const NOTHING: readonly string[] = [];

/** Ids are made in time order: a larger one is newer. */
const newerFirst = (one: TreeNode, other: TreeNode): number => (one.id < other.id ? 1 : one.id > other.id ? -1 : 0);

/**
 * How the rows of one parent are ordered. Places of one kind stand together in every order but the reader's own; within
 * a kind, the newest first, the most recently changed first (with the name between equals), or by name. In the reader's
 * own order the rows they arranged come first as they put them, and the rest after them, the newest first.
 */
function comparerOf(sortedBy: TreeOrder | undefined, parent: string, collator: Intl.Collator): (one: TreeNode, other: TreeNode) => number {
  const byKind = (one: TreeNode, other: TreeNode) => KIND_ORDER[one.kind] - KIND_ORDER[other.kind];
  switch (sortedBy?.by) {
    case 'newest':
      return (one, other) => byKind(one, other) || newerFirst(one, other);
    case 'changed':
      return (one, other) => byKind(one, other)
        || (other.activity_at ?? other.thread?.last_activity_at ?? '').localeCompare(one.activity_at ?? one.thread?.last_activity_at ?? '')
        || other.tree_seq - one.tree_seq || collator.compare(one.name, other.name);
    case 'custom': {
      const put = sortedBy.custom?.[parent] ?? [];
      const at = (node: TreeNode) => {
        const index = put.indexOf(node.id);
        return index < 0 ? put.length : index;
      };
      return (one, other) => at(one) - at(other) || newerFirst(one, other);
    }
    case 'name':
    case undefined:
      return (one, other) => byKind(one, other) || collator.compare(one.name, other.name);
  }
}

/** How the nodes stand now: the last shape when no node has moved, else a new one with its brothers sorted. */
function shapeOf(tree: OrgTree, locale: string, showArchived: boolean, memory: LayoutMemory, sortedBy?: TreeOrder): Shape {
  const held = memory.shape;
  if (held !== null && held.locale === locale && held.showArchived === showArchived && held.sortedBy === sortedBy && held.nodes.size === tree.nodes.size) {
    let moved = false;
    for (const [id, node] of tree.nodes) {
      const before = held.nodes.get(id);
      if (before !== node && (before === undefined || !standsAsBefore(before, node))) {
        moved = true;
        break;
      }
    }
    if (!moved) {
      const same: Shape = { ...held, nodes: tree.nodes };
      memory.shape = same;
      return same;
    }
  }
  const under = new Map<string, TreeNode[]>();
  for (const node of tree.nodes.values()) {
    if (node.deleted || (node.archived && !showArchived) || node.parent_id === null) continue;
    const list = under.get(node.parent_id);
    if (list === undefined) under.set(node.parent_id, [node]);
    else list.push(node);
  }
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  const order = new Map<string, readonly string[]>();
  for (const [parent, list] of under) {
    list.sort(comparerOf(sortedBy, parent, collator));
    order.set(
      parent,
      list.map((node) => node.id),
    );
  }
  const made: Shape = { nodes: tree.nodes, locale, showArchived, sortedBy, order };
  memory.shape = made;
  return made;
}

/** Whether two items show the same, so that the row of the first need not be drawn again for the second. */
function showsTheSame(held: NavItem, made: NavItem): boolean {
  switch (made.type) {
    case 'node':
      return (
        held.type === 'node' &&
        held.node === made.node &&
        held.urgent === made.urgent &&
        held.folder === made.folder &&
        held.waiting === made.waiting &&
        held.unread === made.unread &&
        held.mentions === made.mentions &&
        held.chat === made.chat &&
        held.chatMentions === made.chatMentions
      );
    case 'doc':
      return held.type === 'doc' && held.doc === made.doc && held.nodeId === made.nodeId;
    case 'docs':
      return held.type === 'docs' && held.nodeId === made.nodeId && held.state === made.state;
    case 'chat':
      return held.type === 'chat' && held.node === made.node && held.unread === made.unread && held.mentions === made.mentions && held.quiet === made.quiet;
    case 'groups':
      return held.type === 'groups' && held.count === made.count && held.unread === made.unread && held.mentions === made.mentions;
    case 'group':
      return held.type === 'group' && held.chat === made.chat && held.unread === made.unread && held.mentions === made.mentions && held.quiet === made.quiet;
    case 'outside':
      return held.type === 'outside' && held.node === made.node && held.where === made.where && held.place === made.place;
    case 'private':
      return held.type === 'private' && held.row === made.row;
  }
}

/**
 * The item a row had before when nothing it shows has changed since, so that the row is not drawn again
 * for nothing: a row is drawn again when its item is another one.
 */
function remembered(made: NavItem, memory: LayoutMemory, id: string): NavItem {
  const held = memory.items.get(id);
  if (held !== undefined && showsTheSame(held, made)) return held;
  memory.items.set(id, made);
  return made;
}

/** What a node holds that needs a look. */
export interface Inside {
  /** The decisions that wait in its threads. */
  waiting: number;
  /** Whether anything in it that the reader follows is unread: a thread, or a chat. */
  unread: boolean;
  /** How often the reader is mentioned in the chats of what it holds. */
  mentions: number;
  /** The place of the most urgent state of its threads in `URGENCY`; its length where nothing is urgent. */
  urgency: number;
}

/** Works out, once for each node however many rows ask, what lies under it that needs a look: its own chat is not among it. */
function insideOf(order: Shape['order'], nodes: ReadonlyMap<string, TreeNode>, attention: Attention, chats: ChatStanding): (node: TreeNode) => Inside {
  const known = new Map<string, Inside>();
  const inside = (node: TreeNode): Inside => {
    const held = known.get(node.id);
    if (held !== undefined) return held;
    const sum: Inside = { waiting: 0, unread: false, mentions: 0, urgency: URGENCY.length };
    for (const id of order.get(node.id) ?? NOTHING) {
      const child = nodes.get(id);
      if (child === undefined) continue;
      if (child.thread !== null) {
        sum.waiting += child.thread.pending_approvals;
        if (isUnread(child, nodes, attention)) sum.unread = true;
        const state = threadDisplayState(child.thread.display);
        const at = isThreadUrgency(state) ? URGENCY.indexOf(state) : -1;
        if (at !== -1 && at < sum.urgency) sum.urgency = at;
      }
      const chat = chatStanding(child.chat, chats);
      if (chat.unread) sum.unread = true;
      sum.mentions += chat.mentions;
      const deeper = inside(child);
      sum.waiting += deeper.waiting;
      if (deeper.unread) sum.unread = true;
      sum.mentions += deeper.mentions;
      if (deeper.urgency < sum.urgency) sum.urgency = deeper.urgency;
    }
    known.set(node.id, sum);
    return sum;
  };
  return inside;
}

/** Whether a topic is done and nothing keeps it in view while done topics are hidden: the screen in view, a decision that waits in it, or something unread in it or in its chat. */
function hiddenAsDone(node: TreeNode, view: TreeView, inside: (node: TreeNode) => Inside, chats: ChatStanding): boolean {
  if (view.hideDone !== true || node.topic?.status !== 'done' || view.keep?.has(node.id) === true) return false;
  const held = inside(node);
  if (held.waiting > 0 || held.unread || held.mentions > 0) return false;
  const chat = chatStanding(node.chat, chats);
  return !chat.unread && chat.mentions === 0;
}

/** What narrowing by a name found of a node: that its own name has what was typed, and that something in it has. */
interface Found {
  match: boolean;
  way: boolean;
}

/**
 * Finds, among everything under a node, what has the typed letters in its name, and the ways down to those: a node that
 * is neither is not among what is returned. `shown` says whether a node is in the tree at all (a thread where threads
 * are out, a done topic that is hidden, are not).
 */
function foundBy(order: Shape['order'], nodes: ReadonlyMap<string, TreeNode>, rootId: string, typed: string, shown: (node: TreeNode) => boolean): Map<string, Found> {
  const found = new Map<string, Found>();
  const walk = (among: readonly string[]): boolean => {
    let any = false;
    for (const id of among) {
      const node = nodes.get(id);
      if (node === undefined || !shown(node)) continue;
      const match = matchesQuery(typed, node.name);
      const way = walk(order.get(id) ?? NOTHING);
      if (match || way) {
        found.set(id, { match, way });
        any = true;
      }
    }
    return any;
  };
  walk(order.get(rootId) ?? NOTHING);
  return found;
}

/** The documents of a place in the order they stand in the tree: by title, as the language sorts it. */
function sortedDocs(docs: readonly DocumentRef[], locale: string, memory: LayoutMemory): readonly DocumentRef[] {
  const held = memory.sorted.get(docs);
  if (held?.locale === locale) return held.docs;
  const collator = new Intl.Collator(locale, { numeric: true, sensitivity: 'base' });
  const sorted = [...docs].sort((one, other) => collator.compare(one.title, other.title) || (one.id < other.id ? -1 : 1));
  memory.sorted.set(docs, { locale, docs: sorted });
  return sorted;
}

/**
 * Lays the tree out as the rows that are in view, in order. `rootId` is the place the tree begins at: the
 * root org, or the place its reader narrowed it to; the rows are what lies in it. `expanded` holds the ids
 * of the rows that are open. `memory` holds what the last laying out was made from: a row whose item has
 * not changed keeps the one it had.
 */
export function buildRows(
  source: OrgTree,
  rootId: string,
  expanded: ReadonlySet<string>,
  locale: string,
  view: TreeView = {},
  memory: LayoutMemory = newLayoutMemory(),
): TreeRow<NavItem>[] {
  const tree = view.participating === true ? participatingTree(source) : source;
  const { order } = shapeOf(tree, locale, view.showArchived === true, memory, view.order);
  const nodeOf = (id: string): TreeNode | undefined => tree.nodes.get(id);
  const attention = view.attention ?? NO_ATTENTION;
  const chats = view.chats ?? NO_CHATS;
  const inside = insideOf(order, tree.nodes, attention, chats);
  const docs = view.docs ?? null;
  const chatRows = view.chatRows === true;
  const threads = view.threads !== false;

  const rows: TreeRow<NavItem>[] = [];
  const leaf = (id: string, depth: number, parentRow: string | null, item: NavItem) => {
    rows.push({ id, depth, parentId: parentRow, expandable: false, expanded: false, item: remembered(item, memory, id) });
  };
  if (view.outside !== undefined && view.outside !== null) leaf(OUTSIDE_ROW, 0, null, { type: 'outside', ...view.outside });

  /** Whether a node is in the tree at all, whatever is open. */
  const shown = (node: TreeNode): boolean => (threads || node.kind !== 'thread') && !hiddenAsDone(node, view, inside, chats);
  const typed = (view.filter ?? '').trim();
  const found = typed === '' ? null : foundBy(order, tree.nodes, rootId, typed, shown);

  /**
   * `inProject`: the nodes among which these stand lie in a project, so that a project among them is a folder. `whole`: these
   * lie in something that was found by its name, and are shown as they are; otherwise only what was found among them is.
   */
  const walk = (parentRow: string | null, depth: number, among: readonly string[], inProject: boolean, whole: boolean) => {
    for (const id of among) {
      const node = nodeOf(id);
      if (node === undefined || !shown(node)) continue;
      const hit = found === null || whole ? undefined : found.get(id);
      if (found !== null && !whole && hit === undefined) continue;
      const below = order.get(id) ?? NOTHING;
      // The way down to what was found is open, whatever its reader left it as.
      const open = hit?.way === true || expanded.has(id);
      const chat = chatStanding(node.chat, chats);
      const holdsDocs = docs !== null && node.docs > 0;
      const holdsChat = chatRows && chat.has;
      // Without threads a topic holds no row of its own kind of thing: it opens only for what stands beside it.
      const holdsRows = threads || node.kind !== 'topic' ? below.length > 0 : false;
      const expandable = holdsRows || holdsDocs || holdsChat;
      const isThread = node.kind === 'thread';
      const held = isThread ? null : inside(node);
      // A row hides what is in it while it is closed; a topic whose threads are out of the tree hides them also while it is open.
      const hides = !open || (!threads && node.kind === 'topic');
      // Its own chat has a row of its own on screen while it is open and chats have rows: until then its glyph speaks for it.
      const chatOnScreen = chatRows && open && expandable;
      // Closed, a place says of its own chat what it says of everything else it hides, and the glyph says nothing twice.
      const folded = !open && !isThread;
      // A thread says what it does itself; a topic what the most urgent of its threads does, open or closed; any other
      // row only what it hides.
      const urgent: WorkState | null = isThread ? (node.thread === null ? null : threadDisplayState(node.thread.display)) : held !== null ? (URGENCY[held.urgency] ?? null) : null;
      rows.push({
        id,
        depth,
        parentId: parentRow,
        expandable,
        expanded: open,
        item: remembered(
          {
            type: 'node',
            node,
            urgent,
            folder: node.kind === 'project' && inProject,
            waiting: held === null ? (node.thread?.pending_approvals ?? 0) : hides ? held.waiting : 0,
            // A thread says of itself that it is unread; any other row says it of what it hides.
            unread: held === null ? isUnread(node, tree.nodes, attention) : (hides && held.unread) || (folded && chat.unread),
            mentions: (held !== null && hides ? held.mentions : 0) + (folded ? chat.mentions : 0),
            chat: !chat.has || chatOnScreen ? 'none' : !chat.followed ? 'quiet' : chat.unread && !folded ? 'unread' : 'read',
            chatMentions: chatOnScreen || folded ? 0 : chat.mentions,
          },
          memory,
          id,
        ),
      });
      if (!open) continue;
      const within = found === null || whole || hit?.match === true;
      walk(id, depth + 1, below, node.kind === 'project', within);
      // What stands beside a place is part of what lies in it: on a mere way to something found it is left out.
      if (within) besideOf(node, id, depth + 1);
    }
  };
  /** What stands beside a place after what it holds: the documents written beside it, with the line that says more are on their way or to be had, and its chat. */
  const besideOf = (node: TreeNode, parentRow: string | null, depth: number) => {
    if (docs !== null && node.docs > 0) {
      const known = docs.get(node.id);
      for (const doc of known === undefined ? [] : sortedDocs(known.docs, locale, memory)) leaf(docRowId(doc.id), depth, parentRow, { type: 'doc', doc, nodeId: node.id });
      const state = known === undefined || known.state === 'loading' ? 'loading' : known.state === 'failed' ? 'failed' : known.loadingMore ? 'loadingMore' : known.more ? 'more' : null;
      if (state !== null) leaf(docsRowId(node.id), depth, parentRow, { type: 'docs', nodeId: node.id, state });
    }
    if (!chatRows) return;
    const chat = chatStanding(node.chat, chats);
    const team = view.teamChat === true && node.parent_id === null;
    // A chat nobody wrote in has no row, except the team's: a chat that is only the way to itself is not quiet either.
    if (chat.has || team) leaf(chatRowId(node.id), depth, parentRow, { type: 'chat', node, unread: chat.unread, mentions: chat.mentions, quiet: chat.has && !chat.followed });
  };
  const root = nodeOf(rootId);
  walk(null, 0, order.get(rootId) ?? NOTHING, root?.kind === 'project', false);
  // The place the tree begins at is no row of it: what stands beside it stands at the tree's own level, after everything else.
  // A tree that is narrowed by a name shows what was found and nothing beside it.
  if (root !== undefined && found === null) besideOf(root, null, 0);
  // The org's group chats lie in the org and in no place inside it: one row holds them, where the tree begins at the org itself.
  const groups = chatRows && found === null && root?.parent_id === null ? (view.groups ?? []).filter((chat) => chats.states.get(chat.id)?.hidden !== true) : [];
  if (groups.length > 0) {
    const stood = groups.map((chat) => ({ chat, ...chatStanding({ id: chat.id, seq: chat.last_message_seq, my: chat.my }, chats) }));
    const open = expanded.has(GROUPS_ROW);
    rows.push({
      id: GROUPS_ROW,
      depth: 0,
      parentId: null,
      expandable: true,
      expanded: open,
      item: remembered(
        { type: 'groups', count: stood.length, unread: !open && stood.some((one) => one.unread), mentions: open ? 0 : stood.reduce((sum, one) => sum + one.mentions, 0) },
        memory,
        GROUPS_ROW,
      ),
    });
    if (open) for (const one of stood) leaf(groupRowId(one.chat.id), 1, GROUPS_ROW, { type: 'group', chat: one.chat, unread: one.unread, mentions: one.mentions, quiet: !one.followed });
  }
  // What was remembered of rows that are no longer in view is let go.
  if (memory.items.size > rows.length) {
    const inView = new Set(rows.map((row) => row.id));
    for (const id of memory.items.keys()) if (!inView.has(id)) memory.items.delete(id);
  }
  return rows;
}

/** The rows that are open when a person sees a tree for the first time: orgs and projects, so that topics are in view. */
export function firstExpanded(tree: OrgTree): string[] {
  return [...tree.nodes.values()].filter((node) => node.kind === 'org' || node.kind === 'project').map((node) => node.id);
}

/**
 * The places that are open and say they hold documents, each with how many and the stamp of its row, as `id:count:seq`,
 * sorted: what the tree asks the documents of. A document made, renamed, moved or deleted stamps its place, so a new
 * stamp reads them again even where the count stays. The place the tree begins at is always among them where it holds
 * any: it is no row, and its documents stand at the tree's own level.
 */
export function placesWithDocs(tree: OrgTree, rootId: string, expanded: ReadonlySet<string>, threads = true): string {
  const wanted: string[] = [];
  for (const id of new Set([rootId, ...expanded])) {
    const node = tree.nodes.get(id);
    // A thread that is out of the tree shows no documents either.
    if (node !== undefined && !node.deleted && node.docs > 0 && (threads || node.kind !== 'thread')) wanted.push(`${id}:${node.docs}:${node.tree_seq}`);
  }
  return wanted.sort().join(',');
}

/** Something of the tree that can be unread and gone to: a thread, or the chat of a place. */
export interface Stop {
  /** The row it has, or would have: a thread's id, or the id of a chat's row. */
  id: string;
  kind: 'thread' | 'chat';
  /** The node: the thread itself, or the place whose chat it is. */
  node: TreeNode;
  unread: boolean;
}

/**
 * Every thread of the tree in the order the tree stands in, open rows or not, and with `chats` the chat of every place after
 * what the place holds, each with whether it is unread: what the keys that go to the next thread and to the next unread thing
 * walk through.
 */
export function stopsInOrder(tree: OrgTree, rootId: string, locale: string, view: TreeView = {}, memory: LayoutMemory = newLayoutMemory(), withChats = false): Stop[] {
  const { order } = shapeOf(tree, locale, view.showArchived === true, memory, view.order);
  const attention = view.attention ?? NO_ATTENTION;
  const chats = view.chats ?? NO_CHATS;
  const inside = insideOf(order, tree.nodes, attention, chats);
  const found: Stop[] = [];
  const chatOf = (node: TreeNode) => {
    if (!withChats) return;
    const chat = chatStanding(node.chat, chats);
    if (chat.has) found.push({ id: chatRowId(node.id), kind: 'chat', node, unread: chat.unread || chat.mentions > 0 });
  };
  const walk = (among: readonly string[]) => {
    for (const id of among) {
      const node = tree.nodes.get(id);
      if (node === undefined || hiddenAsDone(node, view, inside, chats)) continue;
      if (node.kind === 'thread') found.push({ id, kind: 'thread', node, unread: isUnread(node, tree.nodes, attention) });
      walk(order.get(id) ?? NOTHING);
      chatOf(node);
    }
  };
  walk(order.get(rootId) ?? NOTHING);
  const root = tree.nodes.get(rootId);
  if (root !== undefined) chatOf(root);
  return found;
}

/** The threads of the tree in the order it stands in, each with whether it is unread. */
export function threadsInOrder(tree: OrgTree, rootId: string, locale: string, view: TreeView = {}, memory: LayoutMemory = newLayoutMemory()): { id: string; unread: boolean }[] {
  return stopsInOrder(tree, rootId, locale, view, memory).map((stop) => ({ id: stop.id, unread: stop.unread }));
}

/** What needs a look in a part of the tree: whether anything in it that the reader follows is unread, the decisions that wait in it, and the times the reader is mentioned in it. */
export interface Needs {
  unread: boolean;
  waiting: number;
  mentions: number;
}

/** What lies directly under each node of a tree, deleted rows left out. */
export type Below = ReadonlyMap<string, readonly TreeNode[]>;

export function belowOf(nodes: ReadonlyMap<string, TreeNode>): Below {
  const below = new Map<string, TreeNode[]>();
  for (const row of nodes.values()) {
    if (row.deleted || row.parent_id === null) continue;
    const list = below.get(row.parent_id);
    if (list === undefined) below.set(row.parent_id, [row]);
    else list.push(row);
  }
  return below;
}

/**
 * What needs a look in the given places and everything in them, their own chats among it: each thread and each chat counted
 * once, however many of the places hold it. Archived things do not count unless they are shown.
 */
export function needsOf(
  tree: Pick<OrgTree, 'nodes'>,
  ids: readonly string[],
  attention: Attention,
  showArchived: boolean,
  chats: ChatStanding = NO_CHATS,
  below: Below = belowOf(tree.nodes),
): Needs {
  const seen = new Set<string>();
  const needs: Needs = { unread: false, waiting: 0, mentions: 0 };
  const pending: TreeNode[] = [];
  for (const id of ids) {
    const row = tree.nodes.get(id);
    if (row !== undefined && !row.deleted) pending.push(row);
  }
  for (let row = pending.pop(); row !== undefined; row = pending.pop()) {
    if (seen.has(row.id) || (row.archived && !showArchived)) continue;
    seen.add(row.id);
    if (row.thread !== null) {
      needs.waiting += row.thread.pending_approvals;
      if (isUnread(row, tree.nodes, attention)) needs.unread = true;
    }
    const chat = chatStanding(row.chat, chats);
    if (chat.unread) needs.unread = true;
    needs.mentions += chat.mentions;
    pending.push(...(below.get(row.id) ?? []));
  }
  return needs;
}

/** The name of a row, by which a typed letter finds it. */
export const nameOf = (item: NavItem): string =>
  item.type === 'node' ? item.node.name : item.type === 'doc' ? item.doc.title : item.type === 'group' ? (item.chat.name ?? '') : item.type === 'private' ? item.row.name : '';
