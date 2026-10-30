/** Folders, which are projects that lie in projects: how the left bar lays them out and what it carries on a closed row, where a project, a folder or a topic can be moved to, what the cards of a project's folders say, and the words for a folder and for how deep they may go. */
import { describe, expect, it } from 'vitest';

import { ApiError } from '@/api/client';
import { deletesDepartment } from '@/features/navigation/NodeDialogs';
import { buildRows, newLayoutMemory, type NavItem } from '@/features/navigation/NodeTree';
import { type NodeMenuActions, type NodeMenuContext, nodeMenuEntries } from '@/features/navigation/NodeMenu';
import { depthWords } from '@/features/navigation/depth';
import { placesAbove } from '@/features/navigation/PlaceChain';
import { isFolderPath, kindWord, type PlaceRef, placeOfNode, placeStats, subtreeCounts } from '@/features/navigation/places';
import { activityLine } from '@/features/org/Activity';
import { destinationsFor } from '@/features/org/Move';
import { folderStays, foldersOf, holdings } from '@/features/project/FolderCards';
import type { ActivityEntry, Node, NodeRef, TreeNodeListedContent as TreeNode } from '@/generated/api-types';
import { i18n } from '@/i18n';
import type { OrgTree } from '@/realtime/stores/tree';
import type { MenuEntry } from '@/ui/Menu';
import type { TreeRow } from '@/ui/Tree';
import { display } from '../doubles/thread-display';

const t = i18n.t.bind(i18n);

const node = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => ({
  id,
  address: null,
  kind,
  content_hidden: false,
  name: id,
  parent_id: parent,
  archived: false,
  deleted: false,
  restricted: false,
  participating: true,
  color: null,
  docs: 0,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
  tree_seq: 1,
  ...more,
});

const thread = (id: string, parent: string, state: NonNullable<TreeNode['thread']>['state'], approvals = 0): TreeNode =>
  node(id, 'thread', parent, { thread: { state, display: display(state === 'starting' ? 'running' : state), blocked_reason: null, pending_approvals: approvals, activity_seq: 1, last_activity_at: '2026-10-11T10:00:00.000Z', attention: { failed: false, uncommitted: null, unpushed: null, stale: true, as_of: null, reason: null } } });

/** Web, with Checkout in it, Payments in that and Refunds in that, and Docs beside Checkout; topics and threads in several of them. */
const rows = (): TreeNode[] => [
  node('acme', 'org', null),
  node('web', 'project', 'acme'),
  node('docs', 'project', 'web'),
  node('checkout', 'project', 'web'),
  node('payments', 'project', 'checkout'),
  node('refunds', 'project', 'payments'),
  node('fix', 'topic', 'web'),
  node('cart', 'topic', 'checkout'),
  node('fees', 'topic', 'payments'),
  node('shipped', 'topic', 'payments', { topic: { status: 'done', tag_ids: [] } }),
  node('infra', 'project', 'acme'),
  thread('fees-main', 'fees', 'waiting_approval', 2),
  thread('shipped-main', 'shipped', 'waiting_approval', 1),
  thread('fix-main', 'fix', 'running'),
  thread('cart-main', 'cart', 'idle'),
];

const treeOf = (nodes: TreeNode[]): OrgTree => ({ nodes: new Map(nodes.map((entry) => [entry.id, entry])), lastSeq: 1, loaded: true, error: null });
const acme = treeOf(rows());

const nodeItem = (row: TreeRow<NavItem>): Extract<NavItem, { type: 'node' }> => {
  if (row.item.type !== 'node') throw new Error('not a node row');
  return row.item;
};
const find = (made: TreeRow<NavItem>[], id: string): TreeRow<NavItem> => {
  const found = made.find((row) => row.id === id);
  if (found === undefined) throw new Error(`no row ${id}`);
  return found;
};
const names = (made: TreeRow<NavItem>[]) => made.map((row) => `${'  '.repeat(row.depth)}${nodeItem(row).node.name}`);
const waiting = (made: TreeRow<NavItem>[], id: string) => nodeItem(find(made, id)).waiting;

describe('the tree of the left bar with folders', () => {
  const everything = new Set(['web', 'checkout', 'payments', 'docs', 'refunds']);

  it('stands folders inside their project to any depth, above its topics, and a finished topic among the others where its name puts it', () => {
    expect(names(buildRows(acme, 'acme', everything, 'en'))).toEqual([
      'infra',
      'web',
      '  checkout',
      '    payments',
      '      refunds',
      '      fees',
      '      shipped',
      '    cart',
      '  docs',
      '  fix',
    ]);
  });

  it('calls a project a folder only where it lies in a project', () => {
    const made = buildRows(acme, 'acme', everything, 'en');
    const folder = (id: string) => nodeItem(find(made, id)).folder;

    expect(['web', 'infra', 'fix'].map(folder)).toEqual([false, false, false]);
    expect(['checkout', 'payments', 'refunds', 'docs'].map(folder)).toEqual([true, true, true, true]);
  });

  it('keeps a project in an org that lies in an org a project, however deep the orgs go', () => {
    const departments = treeOf([node('acme', 'org', null), node('research', 'org', 'acme'), node('ml', 'org', 'research'), node('evals', 'project', 'ml'), node('baseline', 'project', 'evals')]);
    const made = buildRows(departments, 'acme', new Set(['research', 'ml', 'evals']), 'en');

    expect(nodeItem(find(made, 'evals')).folder).toBe(false);
    expect(nodeItem(find(made, 'baseline')).folder).toBe(true);
  });

  it('shows on a row that is closed the approvals that wait in what it hides, and on no row that is open', () => {
    const closed = buildRows(acme, 'acme', new Set(), 'en');
    expect(waiting(closed, 'web')).toBe(3);
    expect(waiting(closed, 'infra')).toBe(0);

    const down = buildRows(acme, 'acme', new Set(['web', 'checkout']), 'en');
    expect(waiting(down, 'web')).toBe(0);
    expect(waiting(down, 'checkout')).toBe(0);
    // Payments is closed, and hides a topic that waits for two approvals and a finished one that waits for one.
    expect(waiting(down, 'payments')).toBe(3);
    expect(waiting(down, 'cart')).toBe(0);
  });

  it('counts on a topic what waits in its threads while they are out of view, for a finished topic as for any other', () => {
    const made = buildRows(acme, 'acme', new Set(['web', 'checkout', 'payments']), 'en');

    expect(waiting(made, 'fees')).toBe(2);
    expect(waiting(made, 'shipped')).toBe(1);
    // Opened, a topic shows its threads, and each thread says what waits in it.
    const opened = buildRows(acme, 'acme', new Set(['web', 'checkout', 'payments', 'shipped']), 'en');
    expect(waiting(opened, 'shipped')).toBe(0);
    expect(nodeItem(find(opened, 'shipped-main')).node.thread?.pending_approvals).toBe(1);
  });

  it('gives a row the item it had when nothing it shows has changed, so that it is not drawn again', () => {
    const memory = newLayoutMemory();
    const before = buildRows(acme, 'acme', everything, 'en', {}, memory);

    // A thread under Payments asks for one more approval: the tree holds that row anew, and every other as it was.
    const changed: OrgTree = { ...acme, nodes: new Map(acme.nodes).set('fees-main', thread('fees-main', 'fees', 'waiting_approval', 3)) };
    const after = buildRows(changed, 'acme', everything, 'en', {}, memory);

    // The topic that hides the thread now carries three; what lies beside it, above it and below it is as it was.
    expect(waiting(after, 'fees')).toBe(3);
    expect(find(after, 'fees').item).not.toBe(find(before, 'fees').item);
    for (const id of ['web', 'checkout', 'payments', 'refunds', 'docs', 'fix', 'cart', 'infra', 'shipped']) {
      expect(find(after, id).item, id).toBe(find(before, id).item);
    }
  });

  it('lets go of what it remembered of a row that is no longer in view', () => {
    const memory = newLayoutMemory();
    buildRows(acme, 'acme', everything, 'en', {}, memory);
    expect(memory.items.has('refunds')).toBe(true);

    buildRows(acme, 'acme', new Set(), 'en', {}, memory);
    expect(memory.items.has('refunds')).toBe(false);
    expect([...memory.items.keys()].sort()).toEqual(['infra', 'web']);
  });

  describe('when it was laid out before', () => {
    const open = new Set(['web', 'checkout', 'payments', 'docs', 'refunds', 'infra']);
    /** The tree with some rows replaced or added, as the live feed holds them. */
    const changing = (...replaced: TreeNode[]): OrgTree => ({ ...acme, nodes: new Map([...acme.nodes, ...replaced.map((entry): [string, TreeNode] => [entry.id, entry])]) });
    const laid = (tree: OrgTree, memory = newLayoutMemory(), showArchived = false) => names(buildRows(tree, 'acme', open, 'en', { showArchived }, memory));

    it('stands a thread whose state changed where it stood, and sorts nothing again', () => {
      const memory = newLayoutMemory();
      const before = laid(acme, memory);
      const shape = memory.shape;

      expect(laid(changing(thread('fix-main', 'fix', 'idle')), memory)).toEqual(before);
      expect(memory.shape?.order).toBe(shape?.order);
    });

    it('stands a node that was renamed, moved, archived or deleted where it stands now', () => {
      const memory = newLayoutMemory();
      laid(acme, memory);

      // Renamed to what sorts before its brothers.
      const renamed = laid(changing(node('docs', 'project', 'web', { name: 'aaa' })), memory);
      expect(renamed.indexOf('  aaa')).toBeLessThan(renamed.indexOf('  checkout'));
      // Moved from Web into Infra: it stands under Infra now, and no longer under Web.
      const moved = laid(changing(node('fix', 'topic', 'infra')), memory);
      expect(moved.indexOf('  fix')).toBeGreaterThan(moved.indexOf('infra'));
      expect(moved.indexOf('  fix')).toBeLessThan(moved.indexOf('web'));
      // Archived, and deleted: out of view.
      expect(laid(changing(node('fix', 'topic', 'web', { archived: true })), memory)).not.toContain('  fix');
      expect(laid(changing(node('fix', 'topic', 'web', { deleted: true })), memory)).not.toContain('  fix');
      // As it was: what a layout made from nothing says.
      expect(laid(acme, memory)).toEqual(laid(acme));
    });

    it('stands a node that is new where it belongs', () => {
      const memory = newLayoutMemory();
      laid(acme, memory);

      const grown = changing(node('brand new', 'topic', 'web'));
      expect(laid(grown, memory)).toContain('  brand new');
      expect(laid(grown, memory)).toEqual(laid(grown));
    });

    it('lays out again when the wish to see archived things changes', () => {
      const memory = newLayoutMemory();
      const withOld = changing(node('old', 'topic', 'web', { archived: true }));

      expect(laid(withOld, memory)).not.toContain('  old');
      expect(laid(withOld, memory, true)).toContain('  old');
      expect(laid(withOld, memory)).not.toContain('  old');
    });
  });
});

describe('where a project, a folder or a topic can be moved to', () => {
  const place = (id: string, kind: PlaceRef['kind'], parentId: string | null, folder = false): PlaceRef => ({ id, kind, name: id, parentId, archived: false, status: null, folder });
  const where = (moved: PlaceRef) => destinationsFor(acme.nodes, moved, ' / ').map((one) => [one.path, one.kind, one.why]);

  it('are, for a folder, every org and every project and folder of the tree but itself and what lies in it, and the place it lies in already is said', () => {
    expect(where(place('payments', 'project', 'checkout', true))).toEqual([
      ['acme', 'org', null],
      ['acme / infra', 'project', null],
      ['acme / web', 'project', null],
      ['acme / web / checkout', 'folder', 'here'],
      ['acme / web / checkout / payments', 'folder', 'inside'],
      ['acme / web / checkout / payments / refunds', 'folder', 'inside'],
      ['acme / web / docs', 'folder', null],
    ]);
  });

  it('are, for a topic, the projects and folders of the tree, those beside the one it lies in among them, and no org', () => {
    expect(where(place('fees', 'topic', 'payments'))).toEqual([
      ['acme / infra', 'project', null],
      ['acme / web', 'project', null],
      ['acme / web / checkout', 'folder', null],
      ['acme / web / checkout / payments', 'folder', 'here'],
      ['acme / web / checkout / payments / refunds', 'folder', null],
      ['acme / web / docs', 'folder', null],
    ]);
  });

  it('leave out what is archived or deleted', () => {
    const fewer = treeOf(rows().map((entry) => (entry.id === 'docs' ? { ...entry, archived: true } : entry.id === 'infra' ? { ...entry, deleted: true } : entry)));

    expect(destinationsFor(fewer.nodes, place('fees', 'topic', 'payments'), ' / ').map((one) => one.path)).not.toContain('acme / web / docs');
    expect(destinationsFor(fewer.nodes, place('fees', 'topic', 'payments'), ' / ').map((one) => one.path)).not.toContain('acme / infra');
  });
});

describe("the cards of a project's folders", () => {
  it('list the folders that lie directly in a project, by name, and the archived ones only when asked', () => {
    expect(foldersOf(acme.nodes, 'web', false, 'en').map((row) => row.id)).toEqual(['checkout', 'docs']);
    expect(foldersOf(acme.nodes, 'checkout', false, 'en').map((row) => row.id)).toEqual(['payments']);

    const archived = treeOf(rows().map((entry) => (entry.id === 'docs' ? { ...entry, archived: true } : entry)));
    expect(foldersOf(archived.nodes, 'web', false, 'en').map((row) => row.id)).toEqual(['checkout']);
    expect(foldersOf(archived.nodes, 'web', true, 'en').map((row) => row.id)).toEqual(['checkout', 'docs']);
  });

  it('say how many topics and folders a folder holds directly, and not what is deeper', () => {
    expect(holdings(acme.nodes, 'payments')).toEqual({ topics: 2, folders: 1 });
    expect(holdings(acme.nodes, 'web')).toEqual({ topics: 1, folders: 2 });
  });

  it('are narrowed by what narrows topics as far as it can reach them: a search by name, what needs a look, and a tag leaves none', () => {
    const none = { running: 0, waiting: 0, blocked: 0 };
    const row = node('payments', 'project', 'checkout');
    const filters = { q: '', tags: 0, attention: null, archived: false } as const;

    expect(folderStays(row, none, filters)).toBe(true);
    expect(folderStays(row, none, { ...filters, q: 'PAY' })).toBe(true);
    expect(folderStays(row, none, { ...filters, q: 'docs' })).toBe(false);
    expect(folderStays(row, none, { ...filters, attention: 'blocked' })).toBe(false);
    expect(folderStays(row, { ...none, blocked: 2 }, { ...filters, attention: 'blocked' })).toBe(true);
    expect(folderStays(row, none, { ...filters, tags: 1 })).toBe(false);
    expect(folderStays(row, none, { ...filters, participating: true })).toBe(true);
    expect(folderStays({ ...row, participating: false }, none, { ...filters, participating: true })).toBe(false);
    expect(folderStays({ ...row, participating: false }, none, filters)).toBe(true);
  });
});

describe('what a card says of a project that holds folders', () => {
  it("counts the project's own open topics and its folders, and what needs a look in everything inside it", () => {
    expect(placeStats(acme.nodes, 'web')).toEqual({
      openTopics: 1,
      folders: 2,
      running: 1,
      waiting: 2,
      approvals: 3,
      blocked: 0,
      lastActivity: '2026-10-11T10:00:00.000Z',
    });
  });

  it('counts folders apart from projects in what deleting a place would take with it', () => {
    expect(subtreeCounts(acme.nodes, 'web')).toEqual({ orgs: 0, projects: 0, folders: 4, topics: 4, threads: 4, running: 1 });
    expect(subtreeCounts(acme.nodes, 'acme')).toMatchObject({ projects: 2, folders: 4 });
  });
});

describe('the place a topic lies in', () => {
  const ref = (id: string, kind: NodeRef['kind']): NodeRef => ({ id, kind, name: id, redacted: false });

  it('is the run of projects and folders just above it, and never an org', () => {
    const path = [ref('acme', 'org'), ref('research', 'org'), ref('web', 'project'), ref('checkout', 'project'), ref('payments', 'project'), ref('fees', 'topic')];

    expect(placesAbove(path).map((one) => one.id)).toEqual(['web', 'checkout', 'payments']);
    expect(placesAbove([ref('acme', 'org'), ref('fees', 'topic')])).toEqual([]);
    expect(placesAbove([])).toEqual([]);
  });

  it('knows a project that lies in a project from the way down to it', () => {
    expect(isFolderPath([ref('acme', 'org'), ref('web', 'project')])).toBe(false);
    expect(isFolderPath([ref('acme', 'org'), ref('web', 'project'), ref('checkout', 'project')])).toBe(true);
    expect(isFolderPath([ref('acme', 'org'), ref('web', 'project'), ref('checkout', 'project'), ref('fees', 'topic')])).toBe(false);
  });

  it('is told to a place made from a node by the way down to it', () => {
    const made = (path: NodeRef[]) => placeOfNode({ ...path.at(-1), id: 'x', kind: 'project', name: 'x', parent_id: 'p', archived: false, path } as unknown as Node);

    expect(made([ref('acme', 'org'), ref('web', 'project')]).folder).toBe(false);
    expect(made([ref('acme', 'org'), ref('web', 'project'), ref('x', 'project')]).folder).toBe(true);
  });

  it('is called a folder in words only for a project that lies in a project', () => {
    expect(kindWord({ kind: 'project', folder: true })).toBe('folder');
    expect(kindWord({ kind: 'project', folder: false })).toBe('project');
    expect(kindWord({ kind: 'topic', folder: false })).toBe('topic');
  });
});

describe('what is said of a folder', () => {
  const entry = (more: Partial<ActivityEntry> = {}): ActivityEntry => ({
    at: '2026-10-11T10:00:00.000Z',
    actor: { id: 'ada', kind: 'human', username: 'ada', display_name: 'Ada Lovelace', avatar: { text: 'A', color: 1 } },
    title: 'Ada Lovelace made the folder Checkout',
    count: 1,
    node: { id: 'checkout', kind: 'project', name: 'Checkout', redacted: false },
    kind: 'history',
    action: 'node.created',
    thread: null,
    item_id: null,
    ...more,
  });

  it('is a folder in a line of Activity where the project lies in a project, and a project where it does not', () => {
    expect(activityLine(entry(), t, true)).toBe('Ada Lovelace made the folder Checkout');
    expect(activityLine(entry(), t, false)).toBe('Ada Lovelace made the project Checkout');
    expect(activityLine(entry({ action: 'node.deleted' }), t, true)).toBe('Ada Lovelace deleted the folder Checkout');
  });

  it('is refused for its depth in the words of the refusal and the number the system gave', () => {
    const refusal = (more: Record<string, unknown>) =>
      new ApiError({ type: 'orch:error:project.too_deep', code: 'project.too_deep', title: 'too deep', status: 409, detail: 'x', request_id: 'r', ...more }, null);

    expect(depthWords(refusal({ limit: 8 }), t)).toBe('Folders cannot be nested any deeper here. Choose a folder nearer the top. A path holds at most 8 projects, one inside another.');
    expect(depthWords(refusal({}), t)).toBeUndefined();
    expect(depthWords(new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: 'x', status: 404, detail: 'x', request_id: 'r' }, null), t)).toBeUndefined();
  });
});

describe('what the menu of a folder and of a topic offers', () => {
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
  const ada = { id: 'ada', kind: 'human' as const, username: 'ada', display_name: 'Ada Lovelace', avatar: { text: 'A', color: 1 } };
  const entries = (kind: PlaceRef['kind'], permissions: string[], more: Partial<NodeMenuContext> = {}, folder = false): MenuEntry[] =>
    nodeMenuEntries(
      {
        place: { id: `${kind}-1`, kind, name: kind === 'topic' ? 'Fees' : 'Payments', parentId: 'parent', archived: false, status: kind === 'topic' ? 'open' : null, folder },
        held: { permissions },
        starred: false,
        where: 'row',
        owner: ada,
        isRoot: false,
        personal: false,
        ...more,
      },
      acts,
      t,
    );
  const ids = (list: readonly MenuEntry[]) => list.flatMap((one) => ('id' in one && one.type !== 'separator' && one.type !== 'label' ? [one.id] : []));

  it('lets whoever opens topics in a project or a folder make a topic or a folder there, and nobody else', () => {
    expect(ids(entries('project', ['project.view', 'project.interact', 'project.create_topics'], {}, true))).toEqual(expect.arrayContaining(['new-topic', 'new-folder']));
    expect(ids(entries('project', ['project.view']))).not.toContain('new-folder');
  });

  it('keeps the act on the screen of a place that is not theirs, switched off with what it takes', () => {
    const shown = entries('project', ['project.view'], { where: 'screen' }, true);
    const folder = shown.find((one) => 'id' in one && one.id === 'new-folder');

    expect(folder).toMatchObject({ disabled: 'You can’t take part in Payments. Ask Ada Lovelace, who owns it.' });
  });

  it('keeps secondary container creation in screen menus without repeating the header primary', () => {
    const project = ids(entries('project', ['project.view', 'project.interact'], { where: 'screen' }));
    expect(project).toContain('new-folder');
    expect(project).not.toContain('new-topic');
    const department = ids(entries('org', ['org.view', 'org.interact', 'org.manage'], { where: 'screen' }));
    expect(department).toContain('new-department');
    expect(department).not.toContain('new-project');
  });

  it('keeps Settings in root and department menus but never offers a personal department', () => {
    for (const where of ['row', 'screen', 'commands'] as const) {
      const settingsHref = '/org/acme.research/settings/general';
      const menu = entries('org', ['org.view', 'org.interact', 'org.manage'], { where, settingsHref, personal: true });
      expect(ids(menu)).not.toContain('new-department');
      expect(menu.find((item) => item.id === 'settings')).toMatchObject({ href: settingsHref });
      expect(ids(menu)).toContain('star');
    }
    expect(ids(entries('org', ['org.view', 'org.interact', 'org.manage'], { where: 'commands', personal: undefined }))).not.toContain('new-department');
  });

  it('offers Move to… to whoever manages a topic, a project or a folder, and to nobody who only works in it', () => {
    expect(ids(entries('topic', ['topic.view', 'topic.interact', 'topic.manage']))).toContain('move');
    expect(ids(entries('topic', ['topic.view', 'topic.interact']))).not.toContain('move');
    expect(ids(entries('project', ['project.view', 'project.manage'], {}, true))).toContain('move');
  });
});

describe('what deleting a place asks of its members and its resources', () => {
  const org = (parent: string | null): Node => ({ kind: 'org', parent_id: parent }) as unknown as Node;

  it('is asked only where an org that lies in another is deleted, since only that has an org above it for them to go to', () => {
    expect(deletesDepartment({ kind: 'org', parentId: 'acme' }, org('acme'))).toBe(true);
    // A root org has no org above it, whatever the place that names it says of where it lies.
    expect(deletesDepartment({ kind: 'org', parentId: 'acme' }, org(null))).toBe(false);
    expect(deletesDepartment({ kind: 'org', parentId: null }, org(null))).toBe(false);
    // Until the org has been read, what the place says is what is known.
    expect(deletesDepartment({ kind: 'org', parentId: 'acme' }, undefined)).toBe(true);
    expect(deletesDepartment({ kind: 'org', parentId: null }, undefined)).toBe(false);
  });

  it('is never asked for a project, a folder, a topic or a thread', () => {
    for (const kind of ['project', 'topic', 'thread'] as const) expect(deletesDepartment({ kind, parentId: 'acme' }, undefined)).toBe(false);
  });
});
