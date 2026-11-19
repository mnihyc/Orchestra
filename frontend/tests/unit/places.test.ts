/** What a place's menu offers to whom, how a project's board and filters read a topic, what an org's rules allow an invitation, and the small rules of the dialogs that make places. */
import { describe, expect, it } from 'vitest';

import { activityLine } from '@/features/org/Activity';
import { durationsFor, initialInvitationKind, invitationRules } from '@/features/org/Invite';
import { isPending } from '@/features/settings/org/Members';
import { destinationsFor } from '@/features/org/Move';
import { type NodeMenuActions, type NodeMenuContext, nodeMenuEntries } from '@/features/navigation/NodeMenu';
import { slugOf, tokenOf } from '@/features/navigation/NodeDialogs';
import { departmentsOf } from '@/features/navigation/OrgSwitcher';
import { buildRows, type NavItem } from '@/features/navigation/NodeTree';
import { type PlaceRef, placeStats, subtreeCounts } from '@/features/navigation/places';
import { columnOf, statusOnDrop } from '@/features/project/TopicBoard';
import { filtersOf, isFiltered } from '@/features/project/Project';
import type { ActivityEntry, Invitation, ResolvedSetting, TreeNodeListedContent as TreeNode } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { abilities, denial, holds, permissionDenial, permissionLabel } from '@/lib/permissions';
import type { OrgTree } from '@/realtime/stores/tree';
import type { MenuEntry } from '@/ui/Menu';
import type { TreeRow } from '@/ui/Tree';
import { display } from '../doubles/thread-display';

const t = i18n.t.bind(i18n);

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

const place = (kind: PlaceRef['kind'], more: Partial<PlaceRef> = {}): PlaceRef => ({
  id: `${kind}-1`,
  kind,
  name: kind === 'topic' ? 'Fix login' : 'Web',
  parentId: 'parent',
  archived: false,
  status: kind === 'topic' ? 'open' : null,
  folder: false,
  ...more,
});

const context = (kind: PlaceRef['kind'], permissions: string[] | undefined, more: Partial<NodeMenuContext> = {}): NodeMenuContext => ({
  place: place(kind),
  held: permissions === undefined ? undefined : { permissions },
  starred: false,
  where: 'row',
  owner: ada,
  isRoot: false,
  personal: false,
  ...more,
});

const ids = (entries: readonly MenuEntry[]): string[] => entries.flatMap((entry) => ('id' in entry && entry.type !== 'separator' && entry.type !== 'label' ? [entry.id] : []));
const entryOf = (entries: readonly MenuEntry[], id: string) => entries.find((entry) => 'id' in entry && entry.id === id);

describe('what the menu of a place offers', () => {
  it('puts Import after Export on places and says when no creation is available', () => {
    const withImport = { ...acts, export: nothing, import: nothing };
    for (const kind of ['org', 'project', 'topic'] as const) {
      const entries = nodeMenuEntries(context(kind, [`${kind}.view`], { importable: false }), withImport, t);
      const names = ids(entries);
      expect(names.indexOf('import')).toBe(names.indexOf('export') + 1);
      expect(entryOf(entries, 'import')).toMatchObject({ disabled: 'You cannot create things here' });
      expect(entryOf(nodeMenuEntries(context(kind, [`${kind}.interact`], { importable: true }), withImport, t), 'import')).not.toHaveProperty('disabled');
    }
    expect(entryOf(nodeMenuEntries(context('thread', ['thread.view'], { importable: true }), withImport, t), 'import')).toBeUndefined();
  });
  it('keeps page-owned creation out of screen menus and preserves it in tree rows in both languages', async () => {
    try {
      for (const locale of ['en', 'zh-CN'] as const) {
        await setLocale(locale);
        for (const kind of ['org', 'project', 'topic'] as const) {
          const rights = [`${kind}.view`, `${kind}.interact`, `${kind}.manage`, 'doc.create', ...(kind === 'project' ? ['project.create_topics'] : kind === 'topic' ? ['topic.start_threads'] : [])];
          const create = { ...acts, newDocument: nothing };
          const onPage = ids(nodeMenuEntries(context(kind, rights, { where: 'screen' }), create, t));
          const screen = onPage.filter((id) => id.startsWith('new-'));
          // New folder is a distinct secondary action, not a copy of the page's New topic.
          expect(screen).toEqual(kind === 'org' ? ['new-department'] : kind === 'project' ? ['new-folder'] : []);
          expect(onPage).toContain('star');
          expect(onPage).toContain('colour');
          if (kind === 'topic') expect(onPage).toContain('done');
          const row = ids(nodeMenuEntries(context(kind, rights), create, t));
          expect(row).toContain('star');
          expect(row).toContain('colour');
          expect(row).toContain('new-document');
          expect(row).toContain(kind === 'org' ? 'new-project' : kind === 'project' ? 'new-topic' : 'new-thread');
          if (kind === 'project') expect(row).toContain('new-folder');
        }
      }
    } finally { await setLocale('en'); }
  });

  it('offers just the creation granted on its own without granting contribution', () => {
    const project = ids(nodeMenuEntries(context('project', ['project.view', 'project.create_topics']), acts, t));
    expect(project).toContain('new-topic');
    expect(project).not.toContain('new-folder');
    const topic = ids(nodeMenuEntries(context('topic', ['topic.view', 'topic.start_threads']), acts, t));
    expect(topic).toContain('new-thread');
    expect(topic).not.toContain('done');
    // Scoped credentials may expose contribution without authorizing the creation permission.
    expect(ids(nodeMenuEntries(context('project', ['project.view', 'project.interact']), acts, t))).not.toContain('new-topic');
    expect(ids(nodeMenuEntries(context('topic', ['topic.view', 'topic.interact']), acts, t))).not.toContain('new-thread');
  });

  it('a viewer can list their eligible kept things without gaining any node mutation', () => {
    const viewer = nodeMenuEntries(context('project', ['project.view']), acts, t);

    expect(ids(viewer)).toEqual(['open', 'open-tab', 'star', 'copy-link', 'deleted']);
  });

  it('offers a project its own acts to whoever manages it, with Delete last and in the colour of danger', () => {
    const entries = nodeMenuEntries(context('project', ['project.view', 'project.interact', 'project.create_topics', 'project.manage']), acts, t);

    expect(ids(entries)).toEqual(
      expect.arrayContaining(['new-topic', 'rename', 'move', 'deleted', 'archive', 'delete']),
    );
    expect(ids(entries).at(-1)).toBe('delete');
    expect(entryOf(entries, 'delete')).toMatchObject({ danger: true });
  });

  it('says on the place’s own screen what an act that is not the caller’s would take, and whom to ask', () => {
    const entries = nodeMenuEntries(context('project', ['project.view'], { where: 'screen' }), acts, t);

    expect(entryOf(entries, 'rename')).toMatchObject({
      disabled: 'You can’t manage Web. Ask Ada Lovelace, who owns it.',
    });
    expect(ids(entries)).not.toContain('open');
  });

  it('does not claim to be complete while nothing is known of what the caller holds', () => {
    const entries = nodeMenuEntries(context('topic', undefined), acts, t);

    expect(ids(entries)).toEqual(['open', 'open-tab', 'star', 'copy-link']);
    expect(entries.at(-1)).toMatchObject({ type: 'label', label: 'Checking what you may do here…' });
  });

  it('reopens a done topic and unarchives an archived one, and offers neither without the right to', () => {
    const rights = ['topic.view', 'topic.interact', 'topic.manage', 'topic.archive'];
    const done = nodeMenuEntries({ ...context('topic', rights), place: place('topic', { status: 'done' }) }, acts, t);

    expect(ids(done)).toContain('reopen');
    expect(ids(done)).not.toContain('done');
    const archived = nodeMenuEntries({ ...context('topic', rights), place: place('topic', { status: 'done', archived: true }) }, acts, t);
    expect(ids(archived)).toContain('unarchive');
    expect(ids(archived)).not.toContain('archive');
    expect(ids(archived)).not.toContain('rename');
    expect(ids(archived)).not.toContain('reopen');
    expect(ids(nodeMenuEntries(context('topic', ['topic.view']), acts, t))).not.toContain('reopen');
  });

  it('archives a root org and a department through their own permission while leaving root moves and deletion out', () => {
    const rights = ['org.view', 'org.interact', 'org.manage', 'members.manage'];
    const root = nodeMenuEntries({ ...context('org', rights), place: place('org', { parentId: null }), isRoot: true }, acts, t);
    const department = nodeMenuEntries(context('org', rights), acts, t);

    expect(ids(root)).toEqual(expect.arrayContaining(['new-project', 'new-department', 'rename', 'archive']));
    expect(ids(root)).not.toContain('move');
    expect(ids(root)).not.toContain('delete');
    expect(ids(department)).toEqual(expect.arrayContaining(['move', 'delete', 'archive']));
    const held = ['org.view', 'org.archive'];
    const archived = nodeMenuEntries({ ...context('org', held), place: place('org', { archived: true }) }, acts, t);
    expect(entryOf(archived, 'unarchive')).toMatchObject({ label: 'Unarchive' });
    expect(ids(archived)).not.toContain('rename');
    expect(ids(nodeMenuEntries(context('org', ['org.view']), acts, t))).not.toContain('archive');
  });

  it('offers Colour to whoever manages a place, and to nobody for a thread, whose icon says what it is doing', () => {
    expect(entryOf(nodeMenuEntries(context('project', ['project.view', 'project.manage']), acts, t), 'colour')).toMatchObject({ label: 'Colour…' });
    expect(entryOf(nodeMenuEntries(context('topic', ['topic.view', 'topic.manage']), acts, t), 'colour')).toBeDefined();
    expect(entryOf(nodeMenuEntries(context('org', ['org.view', 'org.manage']), acts, t), 'colour')).toBeDefined();
    // The named menu repeats the header icon's act and retains its actual refusal reason.
    expect(entryOf(nodeMenuEntries(context('project', ['project.view']), acts, t), 'colour')).toBeUndefined();
    for (const kind of ['project', 'topic'] as const) {
      for (const permissions of [[`${kind}.view`], [`${kind}.view`, `${kind}.manage`]]) {
        const entry = entryOf(nodeMenuEntries(context(kind, permissions, { where: 'screen' }), acts, t), 'colour');
        expect(entry).toMatchObject({ label: 'Colour…' });
        if (permissions.includes(`${kind}.manage`)) expect(entry).not.toHaveProperty('disabled');
        else expect(entry).toHaveProperty('disabled');
      }
    }
    expect(entryOf(nodeMenuEntries(context('org', ['org.view'], { where: 'screen' }), acts, t), 'colour')).toMatchObject({ disabled: 'You can’t manage Web. Ask Ada Lovelace, who owns it.' });
    expect(entryOf(nodeMenuEntries(context('thread', ['thread.view', 'thread.manage']), acts, t), 'colour')).toBeUndefined();
  });

  it('keeps what the tree shows out of the menu of a place: showing archived things is the tree\'s own choice', () => {
    expect(entryOf(nodeMenuEntries(context('project', ['project.view', 'project.manage']), acts, t), 'show-archived')).toBeUndefined();
  });
});

describe('what the caller holds', () => {
  it('is read from the permissions of the node and never from a role', () => {
    const held = { permissions: ['project.view', 'project.interact', 'access.manage'] };

    expect(holds(held, 'project.interact')).toBe(true);
    expect(holds(held, 'project.manage')).toBe(false);
    expect(holds(undefined, 'project.view')).toBe(false);
    expect(abilities('project', held)).toEqual({ interact: true, manage: false, archive: false, members: false, access: true });
  });

  it('lets whoever manages a node archive it, as the row that archives goes with Manage', () => {
    expect(abilities('topic', { permissions: ['topic.manage'] }).archive).toBe(true);
    expect(abilities('topic', { permissions: ['topic.archive'] }).archive).toBe(true);
  });

  it('names a permission in words and reports an unavailable name without showing a protocol key', () => {
    expect(permissionLabel('org.interact')).toBe('Contribute to Org');
    expect(permissionLabel('widget.frobnicate')).toBe(i18n.t('resourceAccessWords.unavailable'));
    expect(permissionLabel('widget.frobnicate')).not.toContain('widget.frobnicate');
    expect(denial(t, 'topic.manage', 'Fix login', null)).toBe('You can’t manage Fix login.');
  });

  it('says what cannot be done and where in either language, without quoted permission names', async () => {
    try {
      await setLocale('en');
      expect(denial(t, 'org.manage', 'Acme', { display_name: 'Ada' })).toBe('You can’t manage Acme. Ask Ada, who owns it.');
      expect(permissionDenial(t, 'settings.manage', 'Acme > dep2')).toBe('You can’t change the settings for Acme > dep2.');
      expect(permissionDenial(t, 'widget.frobnicate', 'Acme > dep2')).toBe('You can’t do this in Acme > dep2.');
      await setLocale('zh-CN');
      expect(denial(t, 'org.manage', 'Acme', { display_name: 'Ada' })).toBe('你目前无法管理Acme。请联系它的所有者Ada。');
      expect(permissionDenial(t, 'files.manage', 'Acme > dep2')).toBe('你目前无法更改Acme > dep2中的文件。');
    } finally {
      await setLocale('en');
    }
  });
});

const topic = (attention: { blocked?: number; running?: number; waiting_approval?: number }, status: 'open' | 'done' = 'open') => ({
  status,
  thread_display_state: (attention.waiting_approval ?? 0) > 0 ? 'waiting_approval' as const : (attention.blocked ?? 0) > 0 ? 'blocked' as const : (attention.running ?? 0) > 0 ? 'running' as const : null,
  attention: { blocked: 0, running: 0, waiting_approval: 0, ...attention },
});

describe("a project's board", () => {
  it('stands a topic in the most urgent state among its threads: waiting for approval, then blocked, running and idle', () => {
    expect(columnOf(topic({ blocked: 1, running: 2 }))).toBe('blocked');
    expect(columnOf(topic({ waiting_approval: 1 }))).toBe('waiting_approval');
    expect(columnOf(topic({ running: 3 }))).toBe('running');
    expect(columnOf(topic({}))).toBe('idle');
  });

  it('puts every done topic in Done, whatever its threads are doing', () => {
    expect(columnOf(topic({ blocked: 1 }, 'done'))).toBe('done');
  });

  it('allows a card to move only into Done, or out of it, because the other columns are derived', () => {
    expect(statusOnDrop('idle', 'done')).toBe('done');
    expect(statusOnDrop('blocked', 'done')).toBe('done');
    expect(statusOnDrop('done', 'idle')).toBe('open');
    expect(statusOnDrop('done', 'running')).toBe('open');
    expect(statusOnDrop('idle', 'running')).toBeNull();
    expect(statusOnDrop('running', 'blocked')).toBeNull();
    expect(statusOnDrop('done', 'done')).toBeNull();
  });
});

describe("what a project's address says of what is shown", () => {
  it('shows the open topics by their latest activity when it says nothing', () => {
    expect(filtersOf({}, 'list')).toEqual({ status: 'open', tags: [], attention: null, sort: 'activity', q: '', archived: 'exclude' });
    expect(isFiltered({})).toBe(false);
  });

  it('shows every status on the board, since its columns are the statuses', () => {
    expect(filtersOf({ status: 'done' }, 'board').status).toBe('all');
  });

  it('asks for archived topics as well when it says so, and counts that as a narrowing of what is shown', () => {
    expect(filtersOf({ archived: true }, 'list').archived).toBe('include');
    expect(isFiltered({ archived: true })).toBe(true);
    expect(isFiltered({ tag: ['bug'] })).toBe(true);
    expect(isFiltered({ q: 'login' })).toBe(true);
    expect(isFiltered({ status: 'open' })).toBe(false);
  });
});

describe("an org's rules for invitations", () => {
  it('starts on Link without mail, Email with mail, and never overrides an org’s allowed forms', () => {
    expect(initialInvitationKind(['link', 'email'], false)).toBe('link');
    expect(initialInvitationKind(['link', 'email'], true)).toBe('email');
    expect(initialInvitationKind(['email'], false)).toBe('email');
    expect(initialInvitationKind(['link'], true)).toBe('link');
  });
  const setting = (key: string, value: unknown): ResolvedSetting => ({ key, value, source: { scope: 'default', node: null }, parts: [], own: null, locked_by: null, writable: true });

  it('are read from its settings, and the system’s own values stand where it says nothing', () => {
    expect(invitationRules(undefined)).toEqual({ forms: ['link', 'email'], defaultHours: 168, maxHours: 720, maxUses: 100 });
    expect(
      invitationRules([setting('invitations.forms', ['email']), setting('invitations.max_hours', 48), setting('invitations.max_uses', 5)]),
    ).toEqual({ forms: ['email'], defaultHours: 168, maxHours: 48, maxUses: 5 });
  });

  it('offer a length of time up to the most and never beyond it', () => {
    expect(durationsFor({ defaultHours: 168, maxHours: 720 })).toEqual([24, 72, 168, 336, 720]);
    expect(durationsFor({ defaultHours: 48, maxHours: 100 })).toEqual([24, 48, 72, 100]);
  });

  it('count an invitation as still good until it is taken back, used up or past its time', () => {
    const invitation = (more: Partial<Invitation>): Invitation =>
      ({ revoked: false, uses: 0, max_uses: 3, expires_at: '2026-12-01T00:00:00.000Z', ...more }) as Invitation;
    const now = Date.parse('2026-10-11T00:00:00.000Z');

    expect(isPending(invitation({}), now)).toBe(true);
    expect(isPending(invitation({ revoked: true }), now)).toBe(false);
    expect(isPending(invitation({ uses: 3 }), now)).toBe(false);
    expect(isPending(invitation({ expires_at: '2026-10-01T00:00:00.000Z' }), now)).toBe(false);
  });
});

const row = (id: string, kind: TreeNode['kind'], parent: string | null, more: Partial<TreeNode> = {}): TreeNode => ({
  id,
  address: null,
  kind,
  content_hidden: false,
  name: id,
  parent_id: parent,
  color: null,
  docs: 0,
  archived: false,
  deleted: false,
  restricted: false,
  participating: true,
  chat: { id: `chat-${id}`, seq: 0 },
  thread: null,
  topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
  tree_seq: 1,
  ...more,
});

const thread = (id: string, parent: string, state: NonNullable<TreeNode['thread']>['state'], approvals = 0, at: string | null = null): TreeNode =>
  row(id, 'thread', parent, { thread: { state, display: display(state === 'starting' ? 'running' : state), blocked_reason: null, pending_approvals: approvals, activity_seq: 1, last_activity_at: at, attention: { failed: false, uncommitted: null, unpushed: null, stale: true, as_of: null, reason: null } } });

const acme = new Map(
  [
    row('acme', 'org', null),
    row('research', 'org', 'acme'),
    row('ml', 'org', 'research'),
    row('web', 'project', 'research'),
    row('fix', 'topic', 'web'),
    row('upgrade', 'topic', 'web'),
    row('shipped', 'topic', 'web', { topic: { status: 'done', tag_ids: [] } }),
    row('old', 'topic', 'web', { archived: true }),
    thread('main', 'fix', 'running', 0, '2026-10-11T09:00:00.000Z'),
    thread('repro', 'fix', 'waiting_approval', 2, '2026-10-11T10:00:00.000Z'),
    thread('stuck', 'upgrade', 'blocked'),
    row('infra', 'project', 'acme'),
  ].map((entry) => [entry.id, entry]),
);

describe('what a card says of a place', () => {
  it('counts the open topics under it, and what needs a look, and says when something last happened', () => {
    expect(placeStats(acme, 'web')).toEqual({ openTopics: 2, folders: 0, running: 1, waiting: 1, approvals: 2, blocked: 1, lastActivity: '2026-10-11T10:00:00.000Z' });
    expect(placeStats(acme, 'infra')).toMatchObject({ openTopics: 0, running: 0, lastActivity: null });
  });

  it('counts everything that deleting a place would take with it', () => {
    expect(subtreeCounts(acme, 'research')).toEqual({ orgs: 1, projects: 1, folders: 0, topics: 4, threads: 3, running: 1 });
  });
});

describe('the places a person can move something to', () => {
  const place = (id: string, kind: PlaceRef['kind'], parentId: string | null): PlaceRef => ({ id, kind, name: id, parentId, archived: false, status: null, folder: false });

  it('are, for a project, every org of the tree and every project in it, read by where each lies, and none of them is a place to put it that it is in already or is itself', () => {
    const found = destinationsFor(acme, place('web', 'project', 'research'), ' / ');

    expect(found.map((one) => [one.path, one.why])).toEqual([
      ['acme', null],
      ['acme / research', 'here'],
      ['acme / research / ml', null],
      ['acme / research / web', 'inside'],
      ['acme / infra', null],
    ]);
  });

  it('are, for a topic, the projects of the tree alone', () => {
    const found = destinationsFor(acme, place('fix', 'topic', 'web'), ' / ');

    expect(found.map((one) => [one.path, one.why])).toEqual([
      ['acme / research / web', 'here'],
      ['acme / infra', null],
    ]);
  });

  it('leave out of an org’s own choices the org itself and everything inside it', () => {
    const found = destinationsFor(acme, place('research', 'org', 'acme'), ' / ');

    expect(found.map((one) => [one.path, one.why])).toEqual([
      ['acme', 'here'],
      ['acme / research', 'inside'],
      ['acme / research / ml', 'inside'],
    ]);
  });

  it('are offered beside the root orgs in the switcher, by where each lies', () => {
    expect(departmentsOf(acme, 'acme').map((one) => one.path)).toEqual([['research'], ['research', 'ml']]);
  });
});

describe('the tree of the left bar', () => {
  const tree: OrgTree = { nodes: acme, lastSeq: 1, loaded: true, error: null };
  const names = (rows: TreeRow<NavItem>[]) => rows.map((entry) => (entry.item.type === 'node' ? entry.item.node.name : 'done'));

  it('leaves archived things out unless the person asked to see them', () => {
    const open = new Set(['research', 'web']);

    expect(names(buildRows(tree, 'acme', open, 'en'))).not.toContain('old');
    expect(names(buildRows(tree, 'acme', open, 'en', { showArchived: true }))).toContain('old');
  });
});

describe('what an activity line says', () => {
  const entry = (action: string | null, more: Partial<ActivityEntry> = {}): ActivityEntry => ({
    at: '2026-10-11T10:00:00.000Z',
    actor: ada,
    title: 'Ada Lovelace renamed Web to Website',
    count: 1,
    node: { id: 'web', kind: 'project', name: 'Website', redacted: false },
    kind: 'history',
    action,
    thread: null,
    item_id: null,
    ...more,
  });

  it('is written in the reader’s language for the acts that make, archive, delete and restore a place', () => {
    expect(activityLine(entry('node.created'), t)).toBe('Ada Lovelace made the project Website');
    expect(activityLine(entry('node.archived'), t)).toBe('Ada Lovelace archived Website');
    expect(activityLine(entry('node.deleted'), t)).toBe('Ada Lovelace deleted the project Website');
  });

  it('uses catalogue words even for an older row without structured wording', () => {
    expect(activityLine(entry('node.updated'), t)).toBe('Ada Lovelace changed Website');
    expect(activityLine(entry(null, { kind: 'turn_completed', title: 'A turn finished' }), t)).toBe('A turn finished in Website');
  });
});

describe('the small rules of making a place', () => {
  it('gives a short name from a name: lower case, letters and digits, hyphens between', () => {
    expect(slugOf('Platform Team')).toBe('platform-team');
    expect(slugOf('  R&D — Lab 2 ')).toBe('r-d-lab-2');
    expect(slugOf('王小明')).toBe('');
    expect(slugOf('a'.repeat(40))).toHaveLength(32);
  });

  it('finds the secret of an invitation in the whole link, in the end of it, and in the secret itself', () => {
    expect(tokenOf('http://127.0.0.1:41872/invite/oinv_abc123?x=1')).toBe('oinv_abc123');
    expect(tokenOf('  /invite/oinv_abc123  ')).toBe('oinv_abc123');
    expect(tokenOf('oinv_abc123')).toBe('oinv_abc123');
  });
});
