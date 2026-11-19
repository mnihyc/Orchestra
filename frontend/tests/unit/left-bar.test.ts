/** The left bar as its rules can be told without a browser: a finished topic that keeps its place, what is unread and what a row that hides it carries, documents and chats in the tree, where the tree begins and what folds away, what the bar keeps for a person and what the device keeps, the colour of an icon, the way to a thing past a place its reader may not see, the marks of the orgs, the kinds of resource with their things, and the numbers on Messages. */
import { describe, expect, it } from 'vitest';

import type { DocumentRef } from '@/api/documents';
import { type Attention, follows, isUnread } from '@/features/navigation/attention';
import {
  beginningFor,
  beginningOf,
  canBeginAt,
  othersFolded,
  placeAbove,
  projectOf,
  readBeginning,
  readOpen,
  wayOpened,
  whereIAm,
  writeBeginning,
  writeOpen,
} from '@/features/navigation/beginning';
import { wayTo } from '@/features/navigation/Breadcrumbs';
import { anotherColour } from '@/features/navigation/Colour';
import { unreadChats } from '@/features/navigation/counts';
import { type DocMenuActions, docMenuEntries } from '@/features/navigation/DocMenu';
import { type NodeMenuActions, nodeMenuEntries } from '@/features/navigation/NodeMenu';
import { orgColor, orgMarks } from '@/features/navigation/OrgSwitcher';
import type { PlaceRef } from '@/features/navigation/places';
import { readChoices, SHIPPED_CHOICES, withChoices } from '@/features/navigation/preferences';
import { resourceRows } from '@/features/navigation/Resources';
import type { NodeDocs } from '@/features/navigation/treeDocs';
import {
  buildRows,
  chatRowId,
  type ChatStanding,
  docRowId,
  groupRowId,
  GROUPS_ROW,
  type NavItem,
  needsOf,
  newLayoutMemory,
  OUTSIDE_ROW,
  placesWithDocs,
  stopsInOrder,
  threadsInOrder,
  type TreeView,
} from '@/features/navigation/treeRows';
import type { ResourceKind } from '@/features/resources/trail';
import type { ChatState, SubscriptionLevel, TreeNode } from '@/generated/api-types';
import { i18n } from '@/i18n';
import type { OrgTree } from '@/realtime/stores/tree';
import type { MenuEntry } from '@/ui/Menu';
import { identityColor } from '@/lib/grapheme';
import { isNodeColor, KIND_COLORS, NODE_COLOR_NAMES, NODE_COLORS, NODE_GLYPHS, nodeColor } from '@/ui/NodeIcon';
import type { TreeRow } from '@/ui/Tree';
import { display } from '../doubles/thread-display';

const t = i18n.t.bind(i18n);

const node = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => ({
  participating: true,
  id,
  address: null,
  kind,
  name: id,
  parent_id: parent,
  color: null,
  docs: 0,
  archived: false,
  deleted: false,
  restricted: false,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
  tree_seq: 1,
  ...more,
});

type ThreadPart = NonNullable<TreeNode['thread']>;
const thread = (id: string, parent: string, more: Partial<ThreadPart> = {}, row: Partial<TreeNode> = {}): TreeNode =>
  node(id, 'thread', parent, { thread: { state: 'idle', display: display(more.state === 'starting' ? 'running' : more.state ?? 'idle'), blocked_reason: null, pending_approvals: 0, activity_seq: 10, last_activity_at: null, attention: { uncommitted: null, unpushed: null, failed: false, stale: true, as_of: null, reason: null }, ...more }, ...row });

const treeOf = (nodes: TreeNode[]): OrgTree => ({ nodes: new Map(nodes.map((entry) => [entry.id, entry])), lastSeq: 1, loaded: true, error: null });

/** Web holds three topics, the middle one finished; Infra holds one. */
const rows = (): TreeNode[] => [
  node('acme', 'org', null),
  node('web', 'project', 'acme'),
  node('alpha', 'topic', 'web'),
  node('beta', 'topic', 'web', { topic: { status: 'done', tag_ids: [] } }),
  node('gamma', 'topic', 'web'),
  thread('alpha-main', 'alpha'),
  thread('beta-main', 'beta'),
  thread('gamma-main', 'gamma'),
  thread('gamma-side', 'gamma'),
  node('infra', 'project', 'acme'),
  node('delta', 'topic', 'infra'),
  thread('delta-main', 'delta'),
];
const acme = treeOf(rows());
const changed = (...replaced: TreeNode[]): OrgTree => ({ ...acme, nodes: new Map([...acme.nodes, ...replaced.map((entry): [string, TreeNode] => [entry.id, entry])]) });

const attention = (read: Record<string, number> = {}, levels: Record<string, SubscriptionLevel> = {}): Attention => ({
  read: new Map(Object.entries(read)),
  levels: new Map(Object.entries(levels)),
});

const nodeItem = (row: TreeRow<NavItem> | undefined): Extract<NavItem, { type: 'node' }> => {
  if (row?.item.type !== 'node') throw new Error('not a node row');
  return row.item;
};
const nameOfRow = (item: NavItem): string => {
  switch (item.type) {
    case 'node':
      return item.node.name;
    case 'doc':
      return `doc ${item.doc.title}`;
    case 'docs':
      return `(${item.state})`;
    case 'chat':
      return `chat of ${item.node.name}`;
    case 'groups':
      return `group chats (${item.count})`;
    case 'group':
      return `group ${item.chat.name ?? ''}`;
    case 'outside':
      return `outside: ${item.node.name} in ${item.where}`;
    case 'private':
      return `private ${item.row.name}`;
  }
};
const names = (made: TreeRow<NavItem>[]) => made.map((row) => `${'  '.repeat(row.depth)}${nameOfRow(row.item)}`);
const lay = (tree: OrgTree, open: string[], view: TreeView = {}) => buildRows(tree, 'acme', new Set(open), 'en', view);
const item = (tree: OrgTree, open: string[], id: string, view: TreeView = {}) => nodeItem(lay(tree, open, view).find((row) => row.id === id));

const chatState = (read: number, more: Partial<ChatState> = {}): ChatState => ({ chat_id: '', last_read_seq: read, read_revision: 0, effective_level: 'all', level: null, hidden: false, engaged: true, mentions_unread: 0, ...more });

describe('a topic that is done', () => {
  it('stands where it stood: nothing in the tree moves when a topic is marked done or opened again', () => {
    const memory = newLayoutMemory();
    const open = new Set(['web', 'infra']);
    const before = names(buildRows(acme, 'acme', open, 'en', {}, memory));
    const order = memory.shape?.order;

    // Alpha is marked done, and Beta is opened again.
    const after = names(buildRows(changed(node('alpha', 'topic', 'web', { topic: { status: 'done', tag_ids: [] } }), node('beta', 'topic', 'web')), 'acme', open, 'en', {}, memory));

    expect(after).toEqual(before);
    expect(before).toEqual(['infra', '  delta', 'web', '  alpha', '  beta', '  gamma']);
    // Nothing was sorted again either: the order of every list is the one that was held.
    expect(memory.shape?.order).toBe(order);
  });

  it('is left out where the reader hides done topics, with its threads', () => {
    expect(names(lay(acme, ['web', 'beta'], { hideDone: true }))).toEqual(['infra', 'web', '  alpha', '  gamma']);
    expect(names(lay(acme, ['web', 'beta']))).toEqual(['infra', 'web', '  alpha', '  beta', '    beta-main', '  gamma']);
  });

  it('stays in view while it is hidden where the screen in view lies in it', () => {
    const view: TreeView = { hideDone: true, keep: new Set(['acme', 'web', 'beta', 'beta-main']) };

    expect(names(lay(acme, ['web', 'beta'], view))).toContain('  beta');
    expect(names(lay(acme, ['web', 'beta'], view))).toContain('    beta-main');
  });

  it('stays in view while it is hidden where something in it still asks for somebody: an approval that waits, or a thread that is unread', () => {
    const waits = changed(thread('beta-main', 'beta', { pending_approvals: 1 }));
    expect(names(lay(waits, ['web'], { hideDone: true }))).toContain('  beta');

    const unread: TreeView = { hideDone: true, attention: attention({ 'beta-main': 3 }) };
    expect(names(lay(acme, ['web'], unread))).toContain('  beta');
    // Read to the end, it is hidden again.
    expect(names(lay(acme, ['web'], { hideDone: true, attention: attention({ 'beta-main': 10 }) }))).not.toContain('  beta');
  });
});

describe('what is unread', () => {
  const nodes = acme.nodes;
  const main = nodes.get('alpha-main')!;

  it('is a thread that has come further than its reader has read', () => {
    expect(isUnread(main, nodes, attention({ 'alpha-main': 9 }))).toBe(true);
    expect(isUnread(main, nodes, attention({ 'alpha-main': 10 }))).toBe(false);
    expect(isUnread(main, nodes, attention({ 'alpha-main': 11 }))).toBe(false);
  });

  it('is not a thread the reader never opened and does not follow', () => {
    expect(isUnread(main, nodes, attention())).toBe(false);
    expect(isUnread(main, nodes, attention({ 'gamma-main': 1 }))).toBe(false);
  });

  it('is a thread the reader never opened where they follow it, itself or a place above it', () => {
    expect(isUnread(main, nodes, attention({}, { 'alpha-main': 'all' }))).toBe(true);
    expect(isUnread(main, nodes, attention({}, { alpha: 'important' }))).toBe(true);
    expect(isUnread(main, nodes, attention({}, { acme: 'all' }))).toBe(true);
    // The nearest level decides: the topic is switched off under an org that is followed.
    expect(isUnread(main, nodes, attention({}, { acme: 'all', alpha: 'off' }))).toBe(false);
    expect(follows('alpha-main', nodes, new Map([['web', 'off'], ['alpha-main', 'all']]))).toBe(true);
  });

  it('is nothing that is archived or gone', () => {
    const read = attention({ 'alpha-main': 1 });
    expect(isUnread({ ...main, archived: true }, nodes, read)).toBe(false);
    expect(isUnread({ ...main, deleted: true }, nodes, read)).toBe(false);
  });

  it('is said by the thread itself, and by the nearest row that is closed above it: said, and never counted', () => {
    const read: TreeView = { attention: attention({ 'gamma-main': 3, 'gamma-side': 3, 'alpha-main': 10 }) };

    // Everything open: the two threads say it, and no row above them does.
    expect(item(acme, ['web', 'gamma'], 'gamma-main', read).unread).toBe(true);
    expect(item(acme, ['web', 'gamma'], 'gamma', read).unread).toBe(false);
    expect(item(acme, ['web', 'gamma'], 'web', read).unread).toBe(false);
    // The topic closed: it says that something in it is unread, however many are.
    expect(item(acme, ['web'], 'gamma', read).unread).toBe(true);
    expect(item(acme, ['web'], 'alpha', read).unread).toBe(false);
    // The project closed: it says it for everything in it.
    expect(item(acme, [], 'web', read).unread).toBe(true);
    expect(item(acme, [], 'infra', read).unread).toBe(false);
    // No row carries a number for unread things: the only numbers are of what waits for the reader in person.
    expect(item(acme, [], 'web', read)).toMatchObject({ waiting: 0, mentions: 0 });
  });

  it('says nothing of what the reader was not given: a thread that left their tree is nothing in a row above', () => {
    const read: TreeView = { attention: attention({ 'gamma-side': 3 }) };
    expect(item(acme, ['web'], 'gamma', read).unread).toBe(true);
    // The side thread is no longer theirs to see: the tree holds a row that says only that it is gone.
    const hidden = changed({ ...thread('gamma-side', 'gamma'), name: '', deleted: true, thread: null });

    expect(item(hidden, ['web'], 'gamma', read).unread).toBe(false);
    expect(item(hidden, [], 'web', read).unread).toBe(false);
    expect(names(lay(hidden, ['web', 'gamma'], read))).not.toContain('    gamma-side');
  });
});

describe('what a place row says goes on in it', () => {
  const busy = changed(thread('alpha-main', 'alpha', { state: 'running' }), thread('gamma-main', 'gamma', { state: 'blocked', blocked_reason: 'provider_exhausted' }), thread('gamma-side', 'gamma', { state: 'waiting_approval', pending_approvals: 2 }));

  it('gives waiting people priority over blockers and running work', () => {
    expect(item(busy, [], 'web').urgent).toBe('waiting_approval');
    expect(item(busy, [], 'web').waiting).toBe(2);
    expect(item(busy, [], 'infra').urgent).toBeNull();
    expect(item(changed(thread('alpha-main', 'alpha', { state: 'running' })), [], 'web').urgent).toBe('running');
  });

  it('retains the same work mark on an expanded project and topic', () => {
    expect(item(busy, ['web'], 'web').urgent).toBe('waiting_approval');
    expect(item(busy, ['web'], 'web').waiting).toBe(0);
    expect(item(busy, ['web'], 'gamma').urgent).toBe('waiting_approval');
    expect(item(busy, ['web', 'gamma'], 'gamma').urgent).toBe('waiting_approval');
    expect(item(busy, ['web'], 'alpha').urgent).toBe('running');
  });
});

const doc = (id: string, title: string, more: Partial<DocumentRef> = {}): DocumentRef =>
  ({ id, title, seq: 1, seen_seq: 1, page: 'follows', archived: false, deleted: false, updated_by: null, node: { id: 'alpha', kind: 'topic', name: 'alpha', redacted: false }, ...more }) as DocumentRef;
const docsOf = (entries: Record<string, Partial<NodeDocs>>): Map<string, NodeDocs> =>
  new Map(Object.entries(entries).map(([id, known]) => [id, { docs: [], more: false, state: 'ready', loadingMore: false, ...known }]));

describe('documents in the tree', () => {
  const withDocs = changed(node('web', 'project', 'acme', { docs: 1 }), node('alpha', 'topic', 'web', { docs: 2 }), thread('alpha-main', 'alpha', {}, { docs: 1 }));
  const known = docsOf({
    web: { docs: [doc('d-road', 'Roadmap')] },
    alpha: { docs: [doc('d-plan', 'Plan'), doc('d-notes', 'canary notes')] },
    'alpha-main': { docs: [doc('d-find', 'Findings')] },
  });

  it('stand under the place they are written beside, after what else it holds, by title', () => {
    expect(names(lay(withDocs, ['web', 'alpha', 'alpha-main'], { docs: known }))).toEqual([
      'infra',
      'web',
      '  alpha',
      '    alpha-main',
      '      doc Findings',
      '    doc canary notes',
      '    doc Plan',
      '  beta',
      '  gamma',
      '  doc Roadmap',
    ]);
  });

  it('let a place be opened that holds nothing else, and are not there at all where the reader switched them off', () => {
    const on = lay(withDocs, ['web', 'alpha'], { docs: known });
    expect(on.find((row) => row.id === 'alpha-main')?.expandable).toBe(true);
    expect(on.find((row) => row.id === 'beta-main')?.expandable).toBeFalsy();

    const off = lay(withDocs, ['web', 'alpha', 'alpha-main'], { docs: null });
    expect(off.find((row) => row.id === 'alpha-main')?.expandable).toBe(false);
    expect(names(off).join()).not.toContain('doc ');
  });

  it('say that they are on their way, that they could not be read, and that there are more than were read', () => {
    const line = (docs: Map<string, NodeDocs>) => names(lay(withDocs, ['web', 'alpha'], { docs })).filter((name) => name.includes('('));

    expect(line(new Map())).toEqual(['    (loading)', '  (loading)']);
    expect(line(docsOf({ alpha: { state: 'failed' }, web: { docs: [doc('d-road', 'Roadmap')], more: true } }))).toEqual(['    (failed)', '  (more)']);
    expect(line(docsOf({ alpha: { docs: [doc('d-plan', 'Plan')], more: true, loadingMore: true }, web: {} }))).toEqual(['    (loadingMore)']);
  });

  it('are asked for only where a place is open and says it holds some', () => {
    expect(placesWithDocs(withDocs, 'acme', new Set(['web', 'alpha', 'gamma', 'nowhere']))).toBe('alpha:2:1,web:1:1');
    expect(placesWithDocs(withDocs, 'acme', new Set())).toBe('');
  });

  it('are read again when their place is stamped though their number stays: one of them was renamed', () => {
    const renamed = changed(node('alpha', 'topic', 'web', { docs: 2, tree_seq: 7 }));
    expect(placesWithDocs(renamed, 'acme', new Set(['alpha']))).toBe('alpha:2:7');
  });

  it('of the root org stand at the tree\'s own level, after everything else: the root is no row that could be opened', () => {
    const rooted = changed(node('acme', 'org', null, { docs: 1 }));
    const handbook = docsOf({ acme: { docs: [doc('d-hand', 'Handbook')] } });

    expect(placesWithDocs(rooted, 'acme', new Set())).toBe('acme:1:1');
    expect(names(lay(rooted, [], { docs: handbook }))).toEqual(['infra', 'web', 'doc Handbook']);
    expect(names(lay(rooted, [], { docs: null }))).toEqual(['infra', 'web']);
  });

  it('keep the row they had while nothing about them changed', () => {
    const memory = newLayoutMemory();
    const open = new Set(['web', 'alpha']);
    const before = buildRows(withDocs, 'acme', open, 'en', { docs: known }, memory);
    const after = buildRows(changed(...withDocs.nodes.values(), thread('gamma-main', 'gamma', { state: 'running' })), 'acme', open, 'en', { docs: known }, memory);

    expect(after.find((row) => row.id === docRowId('d-plan'))?.item).toBe(before.find((row) => row.id === docRowId('d-plan'))?.item);
  });
});

describe('the threads of the tree, as the keys walk them', () => {
  it('are in the order the tree stands in, whatever is open, each with whether it is unread', () => {
    const view: TreeView = { attention: attention({ 'gamma-side': 2 }) };
    expect(threadsInOrder(acme, 'acme', 'en', view)).toEqual([
      { id: 'delta-main', unread: false },
      { id: 'alpha-main', unread: false },
      { id: 'beta-main', unread: false },
      { id: 'gamma-main', unread: false },
      { id: 'gamma-side', unread: true },
    ]);
    expect(threadsInOrder(acme, 'acme', 'en', { hideDone: true }).map((one) => one.id)).not.toContain('beta-main');
  });
});

describe('what a folded section says of its rows', () => {
  it('is whether anything in them is unread, and how many decisions wait, each thread once', () => {
    const waits = changed(thread('gamma-main', 'gamma', { pending_approvals: 2 }), thread('delta-main', 'delta', { pending_approvals: 1 }));
    const read = attention({ 'gamma-main': 1, 'alpha-main': 1 });

    expect(needsOf(waits, ['acme'], read, false)).toEqual({ unread: true, waiting: 3, mentions: 0 });
    // A project and a topic inside it: what the topic holds is not counted twice.
    expect(needsOf(waits, ['web', 'gamma', 'gamma-main'], read, false)).toEqual({ unread: true, waiting: 2, mentions: 0 });
    expect(needsOf(waits, ['infra'], read, false)).toEqual({ unread: false, waiting: 1, mentions: 0 });
    expect(needsOf(waits, ['nowhere'], read, false)).toEqual({ unread: false, waiting: 0, mentions: 0 });
  });

  it('takes the chats in: one that is unread for somebody who follows it, and every mention of the reader', () => {
    const talked = changed(node('alpha', 'topic', 'web', { chat: { id: 'chat-alpha', seq: 7 } }), node('infra', 'project', 'acme', { chat: { id: 'chat-infra', seq: 4 } }));
    const standing: ChatStanding = { states: new Map([['chat-alpha', chatState(2, { mentions_unread: 2 })], ['chat-infra', chatState(1, { effective_level: 'off', mentions_unread: 1 })]]), latest: new Map() };

    expect(needsOf(talked, ['web'], attention(), false, standing)).toEqual({ unread: true, waiting: 0, mentions: 2 });
    // A chat that is not followed is unread for nobody, and still carries the mention.
    expect(needsOf(talked, ['infra'], attention(), false, standing)).toEqual({ unread: false, waiting: 0, mentions: 1 });
  });

  it('leaves out what is archived unless archived things are shown', () => {
    const archived = changed(node('gamma', 'topic', 'web', { archived: true }), thread('gamma-main', 'gamma', { pending_approvals: 2 }, { archived: true }));

    expect(needsOf(archived, ['web'], attention(), false).waiting).toBe(0);
    expect(needsOf(archived, ['web'], attention(), true).waiting).toBe(2);
  });
});

describe('what the left bar keeps for a person', () => {
  it('begins with Recent folded, threads and documents in the tree, no chats, and done topics in view', () => {
    expect(SHIPPED_CHOICES).toEqual({ folded: ['recent'], show: { threads: true, documents: true, chats: false }, hideDone: false });
    expect(readChoices(undefined)).toEqual(SHIPPED_CHOICES);
    expect(readChoices(null)).toEqual(SHIPPED_CHOICES);
    expect(readChoices({})).toEqual(SHIPPED_CHOICES);
  });

  it('reads what was kept, and nothing it does not know', () => {
    expect(readChoices({ folded: [], show: { documents: false, chats: true }, hide_done: true })).toEqual({ folded: [], show: { threads: true, documents: false, chats: true }, hideDone: true });
    expect(readChoices({ folded: ['projects', 'weather', 7, 'starred'] }).folded).toEqual(['starred', 'projects']);
    expect(readChoices({ folded: 'recent', show: { documents: 'yes', weather: true } })).toEqual(SHIPPED_CHOICES);
  });

  it('changes one choice and leaves the others, and whatever else is kept beside them, as they are', () => {
    const show = { threads: true, documents: true, chats: false };
    const kept = { folded: ['recent'], show, hide_done: false, last_org_id: 'acme' };

    expect(withChoices(kept, { hideDone: true })).toEqual({ folded: ['recent'], show, hide_done: true, last_org_id: 'acme' });
    expect(withChoices(kept, { folded: [] })).toEqual({ folded: [], show, hide_done: false, last_org_id: 'acme' });
    // Somebody who never chose anything gets the shipped choices written with their first one.
    expect(withChoices(undefined, { show: { ...show, chats: true } })).toEqual({ folded: ['recent'], show: { threads: true, documents: true, chats: true }, hide_done: false });
  });

  it('keeps nothing of where the tree begins: that belongs to the device', () => {
    expect(Object.keys(withChoices(undefined, {})).sort()).toEqual(['folded', 'hide_done', 'show']);
  });
});

// ---------------------------------------------------------------- colour

describe('the colour of an icon', () => {
  it('is the kind\'s as shipped, another for each kind, and the one somebody gave where they gave one', () => {
    expect(new Set(Object.values(KIND_COLORS)).size).toBe(4);
    expect(Object.values(KIND_COLORS).every(isNodeColor)).toBe(true);
    expect(nodeColor('topic', null)).toBe(KIND_COLORS.topic);
    expect(nodeColor('folder', undefined)).toBe(KIND_COLORS.folder);
    expect(nodeColor('project', 4)).toBe(4);
    // A number that is none of the twelve is the kind's colour, not a thirteenth.
    expect(nodeColor('project', 13)).toBe(KIND_COLORS.project);
    expect(nodeColor('project', 0)).toBe(KIND_COLORS.project);
    // A thread says what it is doing and a document is a page: neither has a colour.
    expect(nodeColor('thread', 5)).toBeNull();
    expect(nodeColor('document', 5)).toBeNull();
  });

  it('is never the only sign of a kind: each kind has a glyph of its own', () => {
    expect(new Set(Object.values(NODE_GLYPHS)).size).toBe(Object.keys(NODE_GLYPHS).length);
  });

  it('picked at random is another of the twelve than the one in use, and every other can come', () => {
    for (let current = 1; current <= NODE_COLORS; current += 1) {
      const picked = Array.from({ length: 11 }, (_, at) => anotherColour(current, () => at / 11));
      expect(picked).not.toContain(current);
      expect(new Set(picked).size).toBe(11);
      expect(picked.every(isNodeColor)).toBe(true);
    }
    // From the colour of its kind every one of the twelve can come; the end of the range is one of them too.
    expect(new Set(Array.from({ length: 12 }, (_, at) => anotherColour(null, () => at / 12))).size).toBe(12);
    expect(isNodeColor(anotherColour(3, () => 0.999999))).toBe(true);
    expect(NODE_COLOR_NAMES).toHaveLength(12);
    expect(NODE_COLORS).toBe(12);
  });
});

describe('the way to the screen in view', () => {
  const none = () => undefined;

  it('is the tree\'s where the tree holds it whole, with what the tree says of each place now', () => {
    const tree = [
      { id: 'acme', kind: 'org' as const, name: 'Acme', parent_id: null },
      { id: 'web', kind: 'project' as const, name: 'Web', parent_id: 'acme' },
      { id: 'shop', kind: 'project' as const, name: 'Shop', parent_id: 'web' },
      { id: 'fix', kind: 'topic' as const, name: 'Fix login', parent_id: 'shop' },
    ];
    const rowOf = (id: string) => (id === 'fix' ? { name: 'Fix the login', color: 4, topic: { status: 'done' } } : id === 'shop' ? { name: 'Shop', color: null, topic: null } : undefined);

    expect(wayTo(tree, undefined, rowOf)).toEqual([
      { id: 'acme', kind: 'org', name: 'Acme', folder: false, color: null, done: false },
      { id: 'web', kind: 'project', name: 'Web', folder: false, color: null, done: false },
      { id: 'shop', kind: 'project', name: 'Shop', folder: true, color: null, done: false },
      { id: 'fix', kind: 'topic', name: 'Fix the login', folder: false, color: 4, done: true },
    ]);
  });

  it('goes on past a place the reader may not see: that place has no name, and the ones after it have theirs', () => {
    // The reader was let into a thread; the tree they hold has that row alone, and its place is not theirs to see.
    const tree = [{ id: 'main', kind: 'thread' as const, name: 'Main thread', parent_id: 'fix' }];
    const own = [
      { id: 'acme', kind: 'org' as const, name: null, redacted: true },
      { id: 'web', kind: 'project' as const, name: null, redacted: true },
      { id: 'shop', kind: 'project' as const, name: null, redacted: true },
      { id: 'fix', kind: 'topic' as const, name: 'Fix login', redacted: false },
      { id: 'main', kind: 'thread' as const, name: 'Main thread', redacted: false },
    ];
    const way = wayTo(tree, own, none);

    expect(way.map((part) => part.name)).toEqual([null, null, null, 'Fix login', 'Main thread']);
    expect(way.map((part) => part.folder)).toEqual([false, false, true, false, false]);
    // Until the node's own way has arrived there is only what the tree holds.
    expect(wayTo(tree, undefined, none).map((part) => part.name)).toEqual(['Main thread']);
  });

  it('gives nothing of a place whose name was left out, even where a tree held knows it by another road', () => {
    const own = [{ id: 'web', kind: 'project' as const, name: null, redacted: true }, { id: 'fix', kind: 'topic' as const, name: 'Fix login', redacted: false }];
    const way = wayTo([], own, () => ({ name: 'Web', color: 7, topic: null }));

    expect(way[0]).toMatchObject({ name: null, color: null });
  });
});

describe('how many of a person\'s chats are unread', () => {
  const state = (chat: string, read: number, more: Partial<ChatState> = {}): ChatState => ({ chat_id: chat, last_read_seq: read, read_revision: 0, effective_level: 'all', level: null, hidden: false, engaged: true, mentions_unread: 0, ...more });
  const chat = (id: string, seq: number, my: ChatState | null = null) => ({ id, last_message_seq: seq, my });

  it('are those they follow whose newest message is beyond what they read', () => {
    const chats = [chat('a', 5, state('a', 5)), chat('b', 9, state('b', 4)), chat('c', 3, state('c', 1, { effective_level: 'off' })), chat('d', 2)];

    expect(unreadChats(chats, new Map(), new Map())).toBe(1);
  });

  it('follow what the person\'s feed said since the list was read: a message that came, and a chat that was read', () => {
    const chats = [chat('a', 5, state('a', 5)), chat('b', 9, state('b', 4))];

    expect(unreadChats(chats, new Map(), new Map([['a', { seq: 6 }]]))).toBe(2);
    expect(unreadChats(chats, new Map([['b', state('b', 9)]]), new Map())).toBe(0);
  });

  it('never count a chat the person hid', () => {
    const chats = [chat('b', 9, state('b', 4))];

    expect(unreadChats(chats, new Map([['b', state('b', 4, { hidden: true })]]), new Map())).toBe(0);
    expect(unreadChats([chat('b', 9, state('b', 4, { hidden: true }))], new Map(), new Map())).toBe(0);
  });
});

describe('the menu of a document in the tree', () => {
  const nothing = (): void => undefined;
  const acts: DocMenuActions = { open: nothing, openInTab: nothing, rename: nothing, move: nothing, share: nothing, export: nothing, requestExport: nothing, copyLink: nothing, remove: nothing };
  const ada = { display_name: 'Ada Lovelace' };
  const ids = (entries: readonly MenuEntry[]) => entries.flatMap((entry) => ('id' in entry && entry.type !== 'separator' ? [entry.id] : []));
  const menu = (permissions: string[] | undefined, archived = false) =>
    ids(docMenuEntries({ archived, owner: ada }, permissions === undefined ? undefined : { access: { permissions } as never }, acts, t));

  it('begins with what a press on the row does, and ends, apart, with what destroys', () => {
    const all = menu(['doc.view', 'doc.edit', 'doc.manage', 'doc.export']);

    expect(all[0]).toBe('open');
    expect(all.at(-1)).toBe('delete');
    expect(all).toEqual(['open', 'open-tab', 'rename', 'move', 'share', 'export', 'copy-link', 'delete']);
  });

  it('leaves out what the reader may not do', () => {
    expect(menu(['doc.view'])).toEqual(['open', 'open-tab', 'share', 'copy-link']);
    expect(menu(['doc.view', 'doc.edit'])).toEqual(['open', 'open-tab', 'rename', 'share', 'copy-link']);
    // Beside an archived place a document is read and not written: its title stays as it is.
    expect(menu(['doc.view', 'doc.edit', 'doc.manage'], true)).not.toContain('rename');
  });

  it('says that it is still asking while what the reader holds has not arrived, and offers only what needs nothing', () => {
    expect(menu(undefined)).toEqual(['open', 'open-tab', 'copy-link', 'checking']);
  });
});

// ---------------------------------------------------------------- chats in the tree

describe('chats in the tree', () => {
  /** Alpha's chat has seven messages, Web's three, the org's five; nobody wrote in the others. */
  const talked = changed(
    node('acme', 'org', null, { chat: { id: 'chat-acme', seq: 5 } }),
    node('web', 'project', 'acme', { chat: { id: 'chat-web', seq: 3 } }),
    node('alpha', 'topic', 'web', { chat: { id: 'chat-alpha', seq: 7 } }),
    node('beta', 'topic', 'web', { topic: { status: 'done', tag_ids: [] }, chat: { id: 'chat-beta', seq: 2 } }),
  );
  const standing = (states: Record<string, ChatState>, latest: Record<string, number> = {}): ChatStanding => ({
    states: new Map(Object.entries(states)),
    latest: new Map(Object.entries(latest).map(([id, seq]) => [id, { seq }])),
  });
  const read = standing({ 'chat-acme': chatState(5), 'chat-web': chatState(3), 'chat-alpha': chatState(7), 'chat-beta': chatState(2) });

  it('have no row and no glyph where nobody wrote, and a glyph on their place while they have no row of their own', () => {
    expect(item(talked, ['web'], 'gamma', { chats: read }).chat).toBe('none');
    expect(item(talked, ['web'], 'alpha', { chats: read }).chat).toBe('read');
    expect(names(lay(talked, ['web', 'alpha'], { chats: read })).join()).not.toContain('chat of');
  });

  it('stand as rows after what a place holds and its documents, where the reader shows chats, and the glyph of an open place goes', () => {
    const withDoc = changed(...talked.nodes.values(), node('alpha', 'topic', 'web', { chat: { id: 'chat-alpha', seq: 7 }, docs: 1 }));
    const view: TreeView = { chats: read, chatRows: true, docs: docsOf({ alpha: { docs: [doc('d-plan', 'Plan')] } }) };

    expect(names(lay(withDoc, ['web', 'alpha'], view))).toEqual(['infra', 'web', '  alpha', '    alpha-main', '    doc Plan', '    chat of alpha', '  beta', '  gamma', '  chat of web', 'chat of acme']);
    // Open, a place's chat is said by its own row; closed, by the place's glyph.
    expect(item(withDoc, ['web', 'alpha'], 'alpha', view).chat).toBe('none');
    expect(item(withDoc, ['web'], 'alpha', view).chat).toBe('read');
    // A topic with nothing in it but its chat opens for that.
    expect(lay(talked, ['web'], view).find((row) => row.id === 'beta')?.expandable).toBe(true);
  });

  it('make a row bold with a dot, and carry a number only for the times the reader is mentioned', () => {
    const unread = standing({ 'chat-alpha': chatState(4), 'chat-web': chatState(3, { mentions_unread: 2 }) });
    const rows = lay(talked, ['web', 'alpha'], { chats: unread, chatRows: true });

    expect(rows.find((row) => row.id === chatRowId('alpha'))?.item).toMatchObject({ unread: true, mentions: 0, quiet: false });
    expect(rows.find((row) => row.id === chatRowId('web'))?.item).toMatchObject({ unread: false, mentions: 2, quiet: false });
    // Closed, the project says that something in it is unread and carries the mentions of its own chat with the rest: its glyph is then only the way to the chat.
    expect(item(talked, [], 'web', { chats: unread, chatRows: true })).toMatchObject({ unread: true, mentions: 2, chat: 'read', chatMentions: 0 });
    // Open, with no rows for chats, the glyph of a place speaks for its chat, and the row does not.
    expect(item(talked, ['web'], 'web', { chats: unread })).toMatchObject({ unread: false, mentions: 0, chat: 'read', chatMentions: 2 });
    // A message that came since the tree was read counts too: on the closed topic's row, and on the glyph of the open one.
    const came = standing({ 'chat-alpha': chatState(7) }, { 'chat-alpha': 8 });
    expect(item(talked, ['web'], 'alpha', { chats: came })).toMatchObject({ unread: true, chat: 'read' });
    expect(item(talked, ['web', 'alpha'], 'alpha', { chats: came })).toMatchObject({ unread: false, chat: 'unread' });
  });

  it('are quiet where the reader does not follow them or has no standing in them: nothing above counts them, and a mention is still carried', () => {
    const quiet = standing({ 'chat-alpha': chatState(1, { effective_level: 'off', mentions_unread: 1 }) });
    const rows = lay(talked, ['web', 'alpha'], { chats: quiet, chatRows: true });

    expect(rows.find((row) => row.id === chatRowId('alpha'))?.item).toMatchObject({ unread: false, mentions: 1, quiet: true });
    // The chat of Web has no state of the reader's at all.
    expect(rows.find((row) => row.id === chatRowId('web'))?.item).toMatchObject({ unread: false, mentions: 0, quiet: true });
    expect(item(talked, ['web'], 'alpha', { chats: quiet })).toMatchObject({ unread: false, mentions: 1, chat: 'quiet', chatMentions: 0 });
    expect(item(talked, ['web', 'alpha'], 'alpha', { chats: quiet })).toMatchObject({ unread: false, mentions: 0, chat: 'quiet', chatMentions: 1 });
    expect(item(talked, [], 'web', { chats: quiet })).toMatchObject({ unread: false, mentions: 1, chat: 'quiet' });
  });

  it('keep a done topic in view while its chat is unread, where done topics are hidden', () => {
    const hidden: TreeView = { hideDone: true, chats: read };
    expect(names(lay(talked, ['web'], hidden))).not.toContain('  beta');

    expect(names(lay(talked, ['web'], { hideDone: true, chats: standing({ 'chat-beta': chatState(1) }) }))).toContain('  beta');
    expect(names(lay(talked, ['web'], { hideDone: true, chats: standing({ 'chat-beta': chatState(2, { effective_level: 'off', mentions_unread: 1 }) }) }))).toContain('  beta');
  });

  it('of the team has its row where the tree begins at a team\'s org, though nothing was said in it yet', () => {
    const view: TreeView = { chats: read, chatRows: true, teamChat: true };

    expect(names(lay(acme, [], view))).toEqual(['infra', 'web', 'chat of acme']);
    expect(lay(acme, [], view).at(-1)?.item).toMatchObject({ quiet: false, unread: false });
    expect(names(lay(acme, [], { chats: read, chatRows: true }))).toEqual(['infra', 'web']);
  });

  it('of groups stand under one row where the tree begins at the org, without the ones the reader hid', () => {
    const groups = [
      { id: 'g-lunch', name: 'Lunch', last_message_seq: 4, my: chatState(1) },
      { id: 'g-ops', name: 'Ops', last_message_seq: 2, my: chatState(2, { mentions_unread: 3 }) },
      { id: 'g-old', name: 'Old', last_message_seq: 9, my: chatState(1) },
    ];
    const view: TreeView = { chats: standing({ 'g-old': chatState(1, { hidden: true }) }), chatRows: true, groups };

    const closed = lay(acme, [], view);
    expect(names(closed)).toEqual(['infra', 'web', 'group chats (2)']);
    expect(closed.at(-1)?.item).toMatchObject({ unread: true, mentions: 3 });
    const open = lay(acme, [GROUPS_ROW], view);
    expect(names(open)).toEqual(['infra', 'web', 'group chats (2)', '  group Lunch', '  group Ops']);
    expect(open.find((row) => row.id === groupRowId('g-lunch'))?.item).toMatchObject({ unread: true, mentions: 0 });
    // Open, the row above them says nothing of them: they say it themselves.
    expect(open.find((row) => row.id === GROUPS_ROW)?.item).toMatchObject({ unread: false, mentions: 0 });
    // Narrowed to a place, the tree holds none of the org's group chats.
    expect(names(buildRows(acme, 'web', new Set(), 'en', view))).toEqual(['alpha', 'beta', 'gamma']);
  });

  it('are stops of the keys that go to what is unread, each after what its place holds', () => {
    const unread = standing({ 'chat-alpha': chatState(4), 'chat-web': chatState(3), 'chat-acme': chatState(5, { mentions_unread: 1 }) });
    const stops = stopsInOrder(talked, 'acme', 'en', { chats: unread, attention: attention({ 'gamma-side': 2 }) }, undefined, true);

    expect(stops.map((stop) => `${stop.id}${stop.unread ? ' *' : ''}`)).toEqual([
      'delta-main',
      'alpha-main',
      'chat:alpha *',
      'beta-main',
      'chat:beta',
      'gamma-main',
      'gamma-side *',
      'chat:web',
      'chat:acme *',
    ]);
    // Without chats the stops are the threads alone.
    expect(stopsInOrder(talked, 'acme', 'en', { chats: unread }).every((stop) => stop.kind === 'thread')).toBe(true);
  });
});

describe('the kinds of thing the tree shows', () => {
  it('without threads is the map of places alone: a topic then opens only for what stands beside it, and still says what its threads do', () => {
    const busy = changed(thread('gamma-side', 'gamma', { state: 'waiting_approval', pending_approvals: 2 }), node('alpha', 'topic', 'web', { docs: 1 }));
    const view: TreeView = { threads: false, attention: attention({ 'gamma-main': 3 }), docs: docsOf({ alpha: { docs: [doc('d-plan', 'Plan')] } }) };
    const rows = lay(busy, ['web', 'alpha', 'gamma'], view);

    expect(names(rows)).toEqual(['infra', 'web', '  alpha', '    doc Plan', '  beta', '  gamma']);
    expect(rows.find((row) => row.id === 'gamma')?.expandable).toBe(false);
    // What the topic hides is said on it though it is open: its threads are not in the tree.
    expect(nodeItem(rows.find((row) => row.id === 'gamma'))).toMatchObject({ unread: true, waiting: 2, urgent: 'waiting_approval' });
    expect(placesWithDocs(changed(thread('alpha-main', 'alpha', {}, { docs: 1 })), 'acme', new Set(['alpha-main']), false)).toBe('');
  });
});

// ---------------------------------------------------------------- where the tree begins

describe('where the tree begins', () => {
  /** Shop is a folder of Web, and Checkout a topic in it; Labs is an org inside Acme with a project of its own. */
  const deep = changed(
    node('shop', 'project', 'web'),
    node('checkout', 'topic', 'shop'),
    thread('checkout-main', 'checkout'),
    node('labs', 'org', 'acme'),
    node('probe', 'project', 'labs'),
    node('old', 'project', 'acme', { archived: true }),
  );

  it('can be an org inside the root, a project, a folder or a topic that is still in view, and nothing else', () => {
    expect(['labs', 'web', 'shop', 'checkout'].every((id) => canBeginAt(deep, 'acme', id))).toBe(true);
    // The root org is where everything begins already; a thread holds nothing a tree could show.
    expect(canBeginAt(deep, 'acme', 'acme')).toBe(false);
    expect(canBeginAt(deep, 'acme', 'checkout-main')).toBe(false);
    expect(canBeginAt(deep, 'acme', 'nowhere')).toBe(false);
    expect(canBeginAt(deep, 'acme', 'old')).toBe(false);
    expect(canBeginAt(deep, 'acme', 'old', true)).toBe(true);
  });

  it('falls back to the org, which shows everything, when the place is gone or was never there', () => {
    expect(beginningOf(deep, 'acme', 'shop')).toBe('shop');
    expect(beginningOf(deep, 'acme', null)).toBeNull();
    expect(beginningOf(changed(...deep.nodes.values(), { ...node('shop', 'project', 'web'), deleted: true }), 'acme', 'shop')).toBeNull();
    // A place under one that is gone is not reached from the root either.
    expect(beginningOf(changed(...deep.nodes.values(), { ...node('shop', 'project', 'web'), deleted: true }), 'acme', 'checkout')).toBeNull();
  });

  it('lays out what lies in the place, at the tree\'s own level, with what stands beside the place after it', () => {
    const view: TreeView = { docs: docsOf({ shop: { docs: [doc('d-spec', 'Spec')] } }) };
    const narrowed = buildRows(changed(...deep.nodes.values(), node('shop', 'project', 'web', { docs: 1 })), 'shop', new Set(['checkout']), 'en', view);

    expect(names(narrowed)).toEqual(['checkout', '  checkout-main', 'doc Spec']);
    // A project that lies in the project the tree begins at is a folder there too.
    expect(nodeItem(buildRows(deep, 'web', new Set(), 'en').find((row) => row.id === 'shop')).folder).toBe(true);
  });

  it('says in its first row where the thing in view lies, while that is outside it', () => {
    const outside = { node: deep.nodes.get('delta')!, where: 'acme / infra', place: 'infra' };
    const rows = buildRows(deep, 'shop', new Set(), 'en', { outside });

    expect(rows[0]).toMatchObject({ id: OUTSIDE_ROW, depth: 0, expandable: false });
    expect(names(rows)).toEqual(['outside: delta in acme / infra', 'checkout']);
    expect(names(buildRows(deep, 'shop', new Set(), 'en', { outside: null }))).toEqual(['checkout']);
  });

  it('is narrowed to where the reader is: the place of the screen in view, and for a thread its topic', () => {
    expect(whereIAm(deep, 'acme', 'checkout', null)).toBe('checkout');
    expect(whereIAm(deep, 'acme', 'shop', null)).toBe('shop');
    expect(whereIAm(deep, 'acme', 'web', null)).toBe('web');
    expect(whereIAm(deep, 'acme', 'labs', null)).toBe('labs');
    expect(whereIAm(deep, 'acme', 'checkout-main', null)).toBe('checkout');
    // A tree that begins somewhere else, or further out, is narrowed to where they are.
    expect(whereIAm(deep, 'acme', 'checkout-main', 'infra')).toBe('checkout');
    expect(whereIAm(deep, 'acme', 'checkout-main', 'web')).toBe('checkout');
    // Where the tree begins there already, there is nothing to do.
    expect(whereIAm(deep, 'acme', 'checkout-main', 'checkout')).toBeNull();
    expect(whereIAm(deep, 'acme', 'web', 'web')).toBeNull();
    // At the org's own screen, or at none of this tree, there is nowhere to narrow to.
    expect(whereIAm(deep, 'acme', 'acme', null)).toBeNull();
    expect(whereIAm(deep, 'acme', null, null)).toBeNull();
    expect(whereIAm(deep, 'acme', 'nowhere', null)).toBeNull();
    // A place that is archived out of view is no place to begin at.
    expect(whereIAm(deep, 'acme', 'old', null)).toBeNull();
    expect(whereIAm(deep, 'acme', 'old', null, true)).toBe('old');
  });

  it('is moved to the thing\'s own project by "Show it there": the outermost project it lies in, or an org inside the root, or everything', () => {
    expect(projectOf(deep, 'acme', 'checkout-main')).toBe('web');
    expect(projectOf(deep, 'acme', 'web')).toBe('web');
    expect(projectOf(deep, 'acme', 'probe')).toBe('probe');
    expect(projectOf(deep, 'acme', 'labs')).toBe('labs');
    expect(projectOf(deep, 'acme', 'acme')).toBeNull();
    expect(projectOf(deep, 'acme', 'nowhere')).toBeNull();
  });

  it('steps one place up at a time, and from a place of the org itself to everything', () => {
    expect(placeAbove(deep, 'acme', 'checkout')).toBe('shop');
    expect(placeAbove(deep, 'acme', 'shop')).toBe('web');
    expect(placeAbove(deep, 'acme', 'web')).toBeNull();
  });

  it('is made the row under the keyboard: the row itself where a tree can begin at it, else the nearest place above it', () => {
    expect(beginningFor(deep, 'acme', 'shop')).toBe('shop');
    expect(beginningFor(deep, 'acme', 'checkout-main')).toBe('checkout');
    expect(beginningFor(deep, 'acme', 'acme')).toBeNull();
    expect(beginningFor(deep, 'acme', 'nowhere')).toBeNull();
  });

  it('walks only what the tree shows when the keys go from thread to thread', () => {
    expect(threadsInOrder(deep, 'shop', 'en').map((one) => one.id)).toEqual(['checkout-main']);
  });
});

describe('a tree narrowed by a name', () => {
  const deep = changed(node('shop', 'project', 'web'), node('checkout', 'topic', 'shop'), thread('checkout-main', 'checkout'));

  it('shows what has the typed letters in its name, with the way down to it open whatever was closed, and nothing else', () => {
    expect(names(lay(deep, [], { filter: 'check' }))).toEqual(['web', '  shop', '    checkout', '      checkout-main']);
    // Without regard to case, and to blanks round what was typed.
    expect(names(lay(deep, [], { filter: '  ALPHA ' }))).toEqual(['web', '  alpha', '    alpha-main']);
    expect(names(lay(deep, [], { filter: 'zzz' }))).toEqual([]);
    // Nothing typed, the tree is whole.
    expect(names(lay(deep, [], { filter: '  ' }))).toEqual(names(lay(deep, [])));
  });

  it('shows what lies in a place that was found as its reader left it: closed, or open with everything in it', () => {
    expect(names(lay(deep, [], { filter: 'infra' }))).toEqual(['infra']);
    expect(lay(deep, [], { filter: 'infra' })[0]).toMatchObject({ expandable: true, expanded: false });
    expect(names(lay(deep, ['infra', 'delta'], { filter: 'infra' }))).toEqual(['infra', '  delta', '    delta-main']);
  });

  it('finds only what the tree shows: no thread where threads are out, no done topic that is hidden', () => {
    expect(names(lay(deep, [], { filter: 'main', threads: false }))).toEqual([]);
    expect(names(lay(deep, [], { filter: 'beta', hideDone: true }))).toEqual([]);
    expect(names(lay(deep, [], { filter: 'beta' }))).toEqual(['web', '  beta', '    beta-main']);
  });

  it('leaves out what stands beside a place that is only on the way, and beside the place the tree begins at', () => {
    const withDocs = changed(...deep.nodes.values(), node('web', 'project', 'acme', { docs: 1 }), node('acme', 'org', null, { docs: 1 }), node('checkout', 'topic', 'shop', { docs: 1 }));
    const docs = docsOf({ web: { docs: [doc('d-road', 'Roadmap')] }, acme: { docs: [doc('d-hand', 'Handbook')] }, checkout: { docs: [doc('d-check', 'Checklist')] } });

    expect(names(lay(withDocs, [], { filter: 'checkout', docs }))).toEqual(['web', '  shop', '    checkout', '      checkout-main', '      doc Checklist']);
  });
});

describe('folding away', () => {
  const deep = changed(node('shop', 'project', 'web'), node('checkout', 'topic', 'shop'), thread('checkout-main', 'checkout'));

  it('closes every branch that does not lead to the row, and keeps what is open inside it', () => {
    const open = new Set(['web', 'infra', 'delta', 'shop', 'checkout', 'alpha', GROUPS_ROW]);

    // The org is on the way to every row, and is no row itself: that it is among the open ones shows nothing.
    expect([...othersFolded(deep, open, 'shop')].sort()).toEqual(['acme', 'checkout', 'shop', 'web']);
    // The way down to the row is opened where it was closed; the row itself stays as it was.
    expect([...othersFolded(deep, new Set(['infra']), 'checkout')].sort()).toEqual(['acme', 'shop', 'web']);
  });

  it('opens the way to a row without closing anything, and the row itself where something stands beside it', () => {
    expect([...wayOpened(deep, new Set(['infra']), 'checkout-main')].sort()).toEqual(['acme', 'checkout', 'infra', 'shop', 'web']);
    expect([...wayOpened(deep, new Set(), 'checkout', true)].sort()).toEqual(['acme', 'checkout', 'shop', 'web']);
    expect(wayOpened(deep, new Set(['web']), 'web').has('web')).toBe(true);
  });
});

describe('what the device keeps of a tree', () => {
  it('is the open rows and the beginning, one of each for every person and org, and nothing where nothing was kept', () => {
    localStorage.clear();
    expect(readOpen('ada', 'acme')).toBeNull();
    expect(readBeginning('ada', 'acme')).toBeNull();

    writeOpen('ada', 'acme', new Set(['web', 'alpha']));
    writeBeginning('ada', 'acme', 'web');
    writeBeginning('ada', 'globex', 'ops');

    expect([...(readOpen('ada', 'acme') ?? [])]).toEqual(['web', 'alpha']);
    expect(readBeginning('ada', 'acme')).toBe('web');
    expect(readBeginning('ada', 'globex')).toBe('ops');
    expect(readBeginning('grace', 'acme')).toBeNull();
    // Showing everything again leaves nothing behind, and the open rows are not touched by it.
    writeBeginning('ada', 'acme', null);
    expect(readBeginning('ada', 'acme')).toBeNull();
    expect(readOpen('ada', 'acme')?.size).toBe(2);
    // Nobody signed in keeps nothing.
    writeBeginning(null, 'acme', 'web');
    expect(readBeginning(null, 'acme')).toBeNull();
  });
});

// ---------------------------------------------------------------- the menu of a place in the tree

describe('what a place\'s menu asks of the tree', () => {
  const nothing = (): void => undefined;
  const acts: NodeMenuActions = {
    open: nothing,
    openInTab: nothing,
    copyLink: nothing,
    star: nothing,
    newProject: nothing,
    newDepartment: nothing,
    newFolder: nothing,
    newTopic: nothing,
    newThread: nothing,
    rename: nothing,
    move: nothing,
    setStatus: nothing,
    archive: nothing,
    unarchive: nothing,
    showDeleted: nothing,
    remove: nothing,
    colour: nothing,
  };
  const web: PlaceRef = { id: 'web', kind: 'project', name: 'Web', parentId: 'acme', archived: false, status: null, folder: false };
  const context = { place: web, held: { permissions: ['project.view'] }, starred: false, where: 'row' as const, owner: null, isRoot: false };
  const entry = (entries: readonly MenuEntry[], id: string) => entries.find((one) => 'id' in one && one.id === id);
  const label = (entries: readonly MenuEntry[], id: string) => {
    const found = entry(entries, id);
    return found !== undefined && 'label' in found ? found.label : undefined;
  };

  it('is to show only it, to collapse the others and to mark it as read, in those words, after Open', () => {
    const entries = nodeMenuEntries({ ...context, unread: true }, { ...acts, showOnly: nothing, collapseOthers: nothing, markRead: nothing }, t);
    const ids = entries.flatMap((one) => ('id' in one && one.type !== 'separator' ? [one.id] : []));

    expect(ids.slice(0, 5)).toEqual(['open', 'open-tab', 'show-only', 'collapse-others', 'mark-read']);
    expect(label(entries, 'show-only')).toBe('Show only this');
    expect(label(entries, 'collapse-others')).toBe('Collapse the others');
    expect(label(entries, 'mark-read')).toBe('Mark as read');
    expect(entry(entries, 'mark-read')).not.toHaveProperty('disabled');
  });

  it('says why there is nothing to mark where nothing is unread, and is not there at all where no tree is drawn', () => {
    const read = nodeMenuEntries({ ...context, unread: false }, { ...acts, markRead: nothing }, t);
    expect(entry(read, 'mark-read')).toMatchObject({ disabled: 'Nothing here is unread.' });

    const screen = nodeMenuEntries(context, acts, t);
    expect(entry(screen, 'show-only')).toBeUndefined();
    expect(entry(screen, 'collapse-others')).toBeUndefined();
    expect(entry(screen, 'mark-read')).toBeUndefined();
  });
});

// ---------------------------------------------------------------- the orgs of the rail

describe('the mark of an org', () => {
  const org = (id: string, name: string, color: number | null = null) => ({ id, name, color });

  it('is the first letter of its name, and the next one with it where another org begins with the same', () => {
    const marks = orgMarks([org('1', 'Ada Lovelace'), org('2', 'Acme'), org('3', 'Globex'), org('4', 'apex labs'), org('5', 'A Team')]);

    expect(marks.get('3')).toBe('G');
    expect(marks.get('1')).toBe('Ad');
    expect(marks.get('2')).toBe('Ac');
    expect(marks.get('4')).toBe('Ap');
    // A space between two words says nothing: the next letter that is written is taken.
    expect(marks.get('5')).toBe('AT');
    expect(orgMarks([org('1', 'Acme')]).get('1')).toBe('A');
    expect(orgMarks([org('1', 'A'), org('2', 'a')]).get('1')).toBe('A1');
  });

  it('distinguishes Beta from Ben’s own space even with the same colour, without changing marks when activity reorders them', () => {
    const team = org('beta', 'Beta', 4);
    const own = org('ben', 'Ben Barker', 4);
    const marks = orgMarks([team, own]);
    expect(marks.get('beta')).toBe('Bt');
    expect(marks.get('ben')).toBe('Bn');
    expect(orgMarks([own, team])).toEqual(marks);
    expect(orgColor(team)).toBe(orgColor(own));
  });

  it('gives exact name twins stable numbers and keeps emoji and CJK characters whole', () => {
    const twins = [org('2', 'Acme'), org('1', 'Acme')];
    expect(orgMarks(twins)).toEqual(new Map([['2', 'Ac2'], ['1', 'Ac1']]));
    expect(orgMarks([...twins].reverse())).toEqual(orgMarks(twins));
    expect(orgMarks([org('lab', '👩🏽‍💻 Lab')]).get('lab')).toBe('👩🏽‍💻');
    expect(orgMarks([org('a', '研究院'), org('b', '研究部')])).toEqual(new Map([['a', '研院'], ['b', '研部']]));
  });

  it('has the colour the org\'s icon was given, and one of its own that stays the same where it was given none', () => {
    expect(orgColor(org('1', 'Acme', 4))).toBe(3);
    expect(orgColor(org('1', 'Acme', 12))).toBe(11);
    expect(orgColor(org('1', 'Acme'))).toBe(identityColor('1'));
    expect(orgColor(org('1', 'Acme'))).toBe(orgColor(org('1', 'Another name')));
  });
});

// ---------------------------------------------------------------- the Resources view

describe('the Resources view of the left bar', () => {
  const none = { things: [], failed: false };
  const lists = {
    providers: { things: [{ id: 'p2', name: 'work key' }, { id: 'p1', name: 'Claude Max' }], failed: false },
    agents: { things: [{ id: 'a1', name: 'Reviewer' }], failed: false },
    machines: none,
    environments: { things: null, failed: false },
    templates: { things: null, failed: true },
    hosts: none,
    mcp: none,
    mounts: none,
    skills: none,
  };
  const names = (open: ResourceKind[]) =>
    resourceRows(lists, new Set(open), 'en').map((row) => `${'  '.repeat(row.depth)}${row.item.type === 'kind' ? `${row.item.kind} ${row.item.count ?? '?'}` : row.item.type === 'thing' ? row.item.name : `(${row.item.state})`}`);

  it('lists every kind there is, in the order the screens of resources name them, each with how many its reader may see', () => {
    expect(names([])).toEqual(['providers 2', 'agents 1', 'machines 0', 'environments ?', 'templates ?', 'hosts 0', 'mcp 0', 'mounts 0', 'skills 0']);
  });

  it('opens a kind to its things by name, says that they are on their way or could not be read, and gives a kind with nothing in it nothing to open', () => {
    expect(names(['providers', 'environments', 'templates', 'machines'])).toEqual([
      'providers 2',
      '  Claude Max',
      '  work key',
      'agents 1',
      'machines 0',
      'environments ?',
      '  (loading)',
      'templates ?',
      '  (failed)',
      'hosts 0',
      'mcp 0',
      'mounts 0',
      'skills 0',
    ]);
    const rows = resourceRows(lists, new Set<ResourceKind>(['machines']), 'en');
    expect(rows.find((row) => row.id === 'kind:machines')).toMatchObject({ expandable: false, expanded: false });
    expect(rows.find((row) => row.id === 'kind:agents')).toMatchObject({ expandable: true, expanded: false });
  });
});
