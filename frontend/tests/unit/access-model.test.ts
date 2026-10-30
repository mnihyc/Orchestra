/** How a permission page reads: who stands in which of the three lines, what the full form lays out and where, what a change names and takes out, and what a refusal for a stale page carries. */
import { describe, expect, it } from 'vitest';

import { currentPageOf } from '@/api/access';
import { ApiError } from '@/api/client';
import {
  addToRowChange,
  type Cell,
  followedPlace,
  gridOf,
  holderKey,
  indexCatalogue,
  isFirstOwner,
  linesOf,
  mayTakeOutOwner,
  nameChange,
  ownersChange,
  removeHolderChange,
  rowOnDefaults,
  rowOnPage,
  wholeChange,
} from '@/features/access/model';
import type { Entry, Holder, NodeOrSystem, PermissionCatalogueGetReply, PermissionPage, PermissionRow, Role, UserRef } from '@/generated/api-types';

import data from '../doubles/catalogue.json';

const catalogue = indexCatalogue(data as unknown as PermissionCatalogueGetReply);

const person = (n: number, name: string): UserRef => ({
  id: `00000000-0000-4000-8001-${n.toString().padStart(12, '0')}`,
  kind: 'human',
  username: name.toLowerCase(),
  display_name: name,
  avatar: { text: name.slice(0, 1), color: n },
});
const ADA = person(1, 'Ada');
const CHEN = person(2, 'Chen');

const role = (name: Role): Holder => ({ kind: 'role', role: name, at: null, count: 0 });
const user = (who: UserRef): Holder => ({ kind: 'user', user: who });
const place = (name: string, id = `00000000-0000-4000-8003-${name.length.toString().padStart(12, '0')}`): NodeOrSystem => ({ id, kind: 'topic', name, redacted: false });

let counter = 0;
const entry = (holder: Holder, more: Partial<Entry> = {}): Entry => {
  counter += 1;
  return {
    id: `00000000-0000-4000-8009-${counter.toString().padStart(12, '0')}`,
    holder,
    options: {},
    outside: false,
    read_first: false,
    expires_at: null,
    note: '',
    source: 'page',
    created_by: ADA,
    created_at: '2026-10-01T09:00:00.000Z',
    revision: 1,
    ...more,
  };
};

/** A row that follows the defaults of a place, which name these roles. */
const follows = (permission: string, roles: readonly Role[], from: NodeOrSystem | null = 'system'): PermissionRow => ({
  permission,
  profile: 'any',
  inherit: true,
  inherited: { holders: roles.map(role), from: roles.length === 0 ? null : from },
  entries: [],
  locked: null,
  writable: true,
  can_give: true,
});

/** The rows of a topic as the system ships them. */
function topicRows(): PermissionRow[] {
  const shipped: Record<string, Role[]> = {
    'chat.view': ['viewer'],
    'chat.interact': ['viewer'],
    'chat.manage': ['developer'],
    'doc.view': ['viewer'],
    'doc.edit': ['interactor'],
    'doc.create': ['viewer'],
    'doc.comment': ['viewer'],
    'doc.manage': ['developer'],
    'topic.view': ['viewer'],
    'topic.interact': ['developer'],
    'topic.manage': ['developer'],
    'topic.archive': [],
    'settings.manage': ['admin'],
    'usage.view': ['admin'],
    'usage.summary': ['viewer'],
    'usage.manage': ['admin'],
    'usage.bypass': ['admin'],
    'audit.view': ['admin'],
    'access.manage': [],
    'access.external': ['developer'],
  };
  return Object.entries(shipped).map(([permission, roles]) => follows(permission, roles));
}

/** The rows of a thread in a managed environment as the system ships them: Interactors talk to the agent. */
function threadRows(): PermissionRow[] {
  const shipped: Record<string, Role[]> = {
    'chat.view': ['viewer'],
    'chat.interact': ['viewer'],
    'chat.manage': ['developer'],
    'doc.view': ['viewer'],
    'doc.edit': ['interactor'],
    'doc.manage': ['developer'],
    'thread.view': ['viewer'],
    'thread.interact': ['interactor'],
    'thread.manage': ['developer'],
    'thread.fork': ['viewer'],
    'thread.archive': [],
  };
  return Object.entries(shipped).map(([permission, roles]) => follows(permission, roles));
}

/** The rows of an environment made from a template as the system ships them: its two levels and the four doors, Browsers and VNC to nobody for reaching in. */
function environmentRows(): PermissionRow[] {
  const shipped: Record<string, Role[]> = {
    'environment.view': ['viewer'],
    'environment.manage': ['developer'],
    'files.view': ['viewer'],
    'files.manage': ['developer'],
    'terminal.view': ['viewer'],
    'terminal.manage': ['developer'],
    'browser.view': ['viewer'],
    'browser.manage': [],
    'vnc.view': ['viewer'],
    'vnc.manage': [],
  };
  return Object.entries(shipped).map(([permission, roles]) => follows(permission, roles));
}

const pageOf = (rows: PermissionRow[], more: Partial<PermissionPage> = {}): PermissionPage => ({
  object: { kind: 'thread', id: '00000000-0000-4000-8003-000000000004', name: 'Fix login' },
  page: 'live',
  state: 'follows',
  revision: 7,
  owner: ADA,
  owners: [entry(user(ADA), { source: 'creation' })],
  owners_writable: true,
  external: true,
  followers: null,
  admins: { count: 1, of: ['system'] },
  depends: [],
  reaches: null,
  covers: null,
  gives: { choices: ['full_view', 'full_interact', 'full_manage'], roles: [], agents: ['view', 'edit'], holders_share: true },
  rows,
  ...more,
});

const names = (list: { holder: Holder }[]): string[] =>
  list.map(({ holder }) => (holder.kind === 'role' ? holder.role : holder.kind === 'user' ? holder.user.display_name : holder.kind));

describe('the three lines of a page', () => {
  it('put each role of a thread in the line of the highest choice it holds whole, and draw only the lowest role of a line', () => {
    const lines = linesOf(pageOf(threadRows()), catalogue, 'thread');
    expect(names(lines.manage)).toEqual(['developer']);
    expect(names(lines.interact)).toEqual(['interactor']);
    // Viewers are named in the rows of View and in two of Interact, which is not the whole of it.
    expect(names(lines.view)).toEqual(['viewer']);
    expect(lines.custom).toEqual([]);
  });

  it('leave a role out of the line below when a role lower than it holds the same, since that one is everybody above it as well', () => {
    // On a topic Developers hold both Interact and Manage: one line, drawn once, in Manage.
    const lines = linesOf(pageOf(topicRows(), { object: { kind: 'topic', id: '00000000-0000-4000-8003-000000000003', name: 'Fix login' } }), catalogue, 'topic');
    expect(names(lines.manage)).toEqual(['developer']);
    expect(lines.interact).toEqual([]);
    expect(names(lines.view)).toEqual(['viewer']);
  });

  it('put a person named with the rows of one choice in that line, and show how they came to be there', () => {
    const named = entry(user(CHEN), { expires_at: '2026-12-31T00:00:00.000Z' });
    const rows = threadRows().map((row) =>
      ['chat.interact', 'doc.edit', 'thread.interact'].includes(row.permission) ? { ...row, entries: [named] } : row,
    );
    const lines = linesOf(pageOf(rows), catalogue, 'thread');
    const chen = lines.interact.find((holder) => holder.holder.kind === 'user');
    expect(chen?.until).toBe('2026-12-31T00:00:00.000Z');
    expect(chen?.entries).toHaveLength(1);
    // A person is not a role: the same entry named in several rows is one chip.
    expect(lines.interact.filter((holder) => holder.holder.kind === 'user')).toHaveLength(1);
  });

  it('list a holder that makes none of the three as Custom, and one that holds nothing of them in no line', () => {
    const only = entry(user(CHEN));
    const rows = threadRows().map((row) => (row.permission === 'chat.manage' ? { ...row, entries: [only] } : row));
    const lines = linesOf(pageOf(rows), catalogue, 'thread');
    expect(lines.custom.map((holder) => holder.holder.kind)).toEqual(['user']);

    // Somebody named in an advanced row alone is on no line: that row is shown under Advanced.
    const fork = threadRows().map((row) => (row.permission === 'thread.fork' ? { ...row, entries: [entry(user(CHEN))] } : row));
    const none = linesOf(pageOf(fork), catalogue, 'thread');
    expect([...none.manage, ...none.interact, ...none.view, ...none.custom].some((holder) => holder.holder.kind === 'user')).toBe(false);
  });

  it('count a row that a lock above decides for what the lock gives, whatever was stored under it', () => {
    const stored = entry(user(CHEN));
    const rows = threadRows().map((row) =>
      row.permission === 'thread.view' ? { ...row, entries: [stored], locked: { by: place('Acme'), here: false }, inherited: { holders: [role('admin')], from: place('Acme') } } : row,
    );
    const lines = linesOf(pageOf(rows), catalogue, 'thread');
    // Chen is named in the row, but the lock above says admins alone, so Chen holds nothing of it.
    expect([...lines.manage, ...lines.interact, ...lines.view, ...lines.custom].some((holder) => holder.holder.kind === 'user')).toBe(false);
  });

  it('keep the roles of one place apart from the roles of another, and never fold one into the other', () => {
    const elsewhere: Holder = { kind: 'role', role: 'viewer', at: { id: '00000000-0000-4000-8003-0000000000aa', kind: 'org', name: 'Globex', redacted: false }, count: 0 };
    const named = entry(elsewhere);
    const rows = threadRows().map((row) => (['chat.view', 'doc.view', 'thread.view'].includes(row.permission) ? { ...row, entries: [named] } : row));
    const lines = linesOf(pageOf(rows), catalogue, 'thread');
    // Both are Viewers, and neither is everybody the other is: two chips, one for each place.
    expect(lines.view.map((holder) => holderKey(holder.holder)).sort()).toEqual([holderKey(role('viewer')), holderKey(elsewhere)].sort());
  });
});

describe('the rows of a page as a grid', () => {
  it('put the rows of one act apart from the levels and the rows about running the place under Administration', () => {
    const page = pageOf(threadRows().concat(follows('settings.manage', ['admin']), follows('usage.summary', ['viewer']), follows('usage.bypass', ['admin']), follows('access.external', ['developer']), follows('access.manage', [])));
    const grid = gridOf(catalogue.of('thread'), rowOnPage(page));
    // A thread's page has no row for files or a terminal: they are the environment's, on its own page.
    expect(grid.areas.map((area) => area.area)).toEqual(['chat', 'doc', 'thread']);
    expect(grid.advanced.map((cell: Cell) => cell.def.name)).toEqual(['thread.fork', 'thread.archive', 'usage.bypass', 'access.external']);
    // The figures of usage belong with administration, with the permission to read the usage.
    expect(grid.administration.map((cell: Cell) => cell.def.name)).toEqual(expect.arrayContaining(['settings.manage', 'usage.summary', 'access.manage']));
  });

  it('give the environment and its four doors no middle cell: there is looking, and there is reaching in', () => {
    const grid = gridOf(catalogue.of('environment'), rowOnPage(pageOf(environmentRows(), { object: { kind: 'environment', id: '00000000-0000-4000-8024-000000000001', name: 'Main' } })));
    expect(grid.areas.map((area) => area.area)).toEqual(['environment', 'files', 'terminal', 'browser', 'vnc']);
    for (const area of grid.areas) expect(area.middle, area.area).toBeNull();
    expect(grid.areas.find((area) => area.area === 'files')?.view?.def.name).toBe('files.view');
    expect(grid.areas.find((area) => area.area === 'vnc')?.manage?.def.name).toBe('vnc.manage');
  });

  it('give a thread its three choices without files and a terminal, which are the environment’s', () => {
    expect(catalogue.choice('full_manage', 'thread')).toEqual(['chat.manage', 'doc.manage', 'thread.manage']);
    expect(catalogue.choice('full_view', 'thread')).not.toEqual(expect.arrayContaining(['files.view']));
    // An environment is looked at, with its doors, or managed: it has no middle choice.
    expect(catalogue.choice('full_view', 'environment')).toEqual(['environment.view', 'files.view', 'terminal.view', 'browser.view', 'vnc.view']);
    expect(catalogue.choice('full_interact', 'environment')).toEqual([]);
    // Documents are edited, not interacted with: their middle cell is the row of Edit.
    const grid = gridOf(catalogue.of('thread'), rowOnPage(pageOf(threadRows())));
    expect(grid.areas.find((area) => area.area === 'doc')?.middle?.def.name).toBe('doc.edit');
  });

  it('read a defaults page for the kind of environment asked for, and the one row where a permission has only one', () => {
    const managed: PermissionRow = { ...follows('thread.interact', ['interactor']), profile: 'managed' };
    const machine: PermissionRow = { ...follows('thread.interact', []), profile: 'connected' };
    const shared: PermissionRow = follows('chat.view', ['viewer']);
    const defaults = pageOf([managed, machine, shared], { page: 'defaults' });
    expect(rowOnDefaults(defaults, 'managed')('thread.interact')?.inherited?.holders).toHaveLength(1);
    expect(rowOnDefaults(defaults, 'connected')('thread.interact')?.inherited?.holders).toHaveLength(0);
    expect(rowOnDefaults(defaults, 'connected')('chat.view')).toBe(shared);
  });
});

describe('where a page stands against its place', () => {
  it('say the place most of its rows follow, and none where it follows nobody', () => {
    const acme = place('Acme');
    const rows = [follows('chat.view', ['viewer'], acme), follows('chat.interact', ['viewer'], acme), follows('chat.manage', ['developer'], 'system')];
    expect(followedPlace(rows)).toBe(acme);
    expect(followedPlace([follows('topic.archive', [])])).toBeNull();
  });
});

describe('the change a page makes', () => {
  it('names holders in the rows of a choice against the revision that was read, and an end when one was set', () => {
    const page = pageOf(threadRows());
    const change = nameChange(page, 'full_interact', [{ kind: 'user', id: CHEN.id }], { expires_at: '2026-12-31T00:00:00.000Z' });
    expect(change).toEqual({
      expected_revision: 7,
      name: [{ choice: 'full_interact', holders: [{ kind: 'user', id: CHEN.id }], expires_at: '2026-12-31T00:00:00.000Z' }],
    });
  });

  it('takes a holder out of its rows, preserving the other followed holders there and leaving other rows alone', () => {
    const named = entry(user(CHEN));
    const rows = threadRows().map((row) => (['thread.view', 'chat.view'].includes(row.permission) ? { ...row, entries: [named] } : row));
    const page = pageOf(rows);
    const holder = linesOf(page, catalogue, 'thread').custom.concat(linesOf(page, catalogue, 'thread').view).find((one) => one.holder.kind === 'user');
    expect(holder).toBeDefined();
    if (holder === undefined) return;
    expect(removeHolderChange(page, holder)).toEqual({
      expected_revision: 7,
      rows: [
        { permission: 'chat.view', inherit: false, add: [{ holder: { kind: 'role', role: 'viewer' } }], remove: [named.id] },
        { permission: 'thread.view', inherit: false, add: [{ holder: { kind: 'role', role: 'viewer' } }], remove: [named.id] },
      ],
    });
  });

  it('cuts one row loose or ties it back, for one kind of environment on a defaults page', () => {
    const page = pageOf(threadRows());
    expect(addToRowChange(page, 'thread.fork', 'any', { kind: 'role', role: 'developer' })).toEqual({
      expected_revision: 7,
      rows: [{ permission: 'thread.fork', add: [{ holder: { kind: 'role', role: 'developer' } }] }],
    });
    expect(wholeChange(page, 'private')).toEqual({ expected_revision: 7, whole: 'private' });
  });
});

describe('the Owners line', () => {
  const second = entry(user(CHEN));
  const group = entry({ kind: 'group', group: { id: '00000000-0000-4000-8008-000000000001', name: 'Backend', description: '', member_count: 3, org_id: '00000000-0000-4000-8002-000000000064' } });

  it('says which owner is first, and takes out nobody who is the only person there, a group beside them or not', () => {
    const only = pageOf([], { owners: [entry(user(ADA)), group] });
    expect(isFirstOwner(only, only.owners[0]!)).toBe(true);
    expect(mayTakeOutOwner(only, only.owners[0]!)).toBe(false);
    // A group can always be taken out, while a person is there.
    expect(mayTakeOutOwner(only, group)).toBe(true);

    const two = pageOf([], { owners: [entry(user(ADA)), second] });
    expect(mayTakeOutOwner(two, second)).toBe(true);
    expect(isFirstOwner(two, second)).toBe(false);
  });

  it('names an owner, takes one out and says who is first in one change', () => {
    expect(ownersChange(pageOf([]), { add: [{ kind: 'user', id: CHEN.id }], first: CHEN.id })).toEqual({
      expected_revision: 7,
      owners: { add: [{ kind: 'user', id: CHEN.id }], first: CHEN.id },
    });
  });
});

describe('a refusal for a page that moved', () => {
  const refused = (current: unknown) =>
    new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision', title: 'Stale', status: 409, detail: 'changed', request_id: 'r1', current }, null);

  it('carries the page as it is now, which is what is shown in place of the one that was seen', () => {
    const now = pageOf(threadRows(), { revision: 9 });
    expect(currentPageOf(refused(now))?.revision).toBe(9);
  });

  it('carries none when the server did not say, and none for any other refusal', () => {
    expect(currentPageOf(refused(undefined))).toBeNull();
    expect(currentPageOf(refused(8))).toBeNull();
    expect(currentPageOf(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'No', status: 403, detail: 'no', request_id: 'r2' }, null))).toBeNull();
  });
});
