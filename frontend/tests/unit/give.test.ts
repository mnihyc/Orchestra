/** Giving: what the reader of a page may give and why they may not give the rest, what sharing with agents comes to in each area, and what letting people into a thread's environment gives and no more. */
import type { TFunction } from 'i18next';
import { describe, expect, it } from 'vitest';

import { areasOf, levelOfParts, mayGiveParts, partsOfLevel, readOnly, sameParts, stepOf, withStep } from '@/features/access/agentShares';
import { askToLetIn, letInChange, offerFor } from '@/features/access/doors';
import { firstChoice, highestChoice, highestRole, keptLine, mayGiveAsRole, mayGiveChoice, sharingIsKept, whyNot } from '@/features/access/gives';
import type { Dependency, PageGives, PermissionPage, UserRef } from '@/generated/api-types';

/** A `t` that says which text was asked for and with what, so that a test can read which sentence a reader gets. */
const t = ((key: string, values?: Record<string, unknown>) => (values === undefined ? key : `${key} ${JSON.stringify(values)}`)) as unknown as TFunction;

const ADA: UserRef = { id: '00000000-0000-4000-8001-000000000001', kind: 'human', username: 'ada', display_name: 'Ada Lovelace', avatar: { text: 'A', color: 1 } };

function page(gives: Partial<PageGives>, more: Partial<PermissionPage> = {}): PermissionPage {
  return {
    object: { kind: 'project', id: '00000000-0000-4000-8003-000000000002', name: 'Web' },
    page: 'live',
    state: 'follows',
    revision: 3,
    owner: ADA,
    owners: [{ id: '00000000-0000-4000-8009-000000000001', holder: { kind: 'user', user: ADA }, options: {}, read_first: false, outside: false, expires_at: null, note: '', source: 'creation', created_by: ADA, created_at: '2026-10-01T09:00:00.000Z', revision: 1 }],
    owners_writable: false,
    external: false,
    followers: null,
    admins: { count: 1, of: ['system'] },
    depends: [],
    reaches: null,
    covers: null,
    gives: { choices: [], roles: [], agents: [], holders_share: true, ...gives },
    rows: [],
    ...more,
  };
}

const words = { t, name: 'Web', kind: 'project', choice: (choice: string) => choice };

describe('what the reader of a page may give', () => {
  it('is the highest of the three choices they may give, and none for somebody who may give none', () => {
    expect(highestChoice(page({ choices: ['full_view', 'full_interact'] }))).toBe('full_interact');
    expect(highestChoice(page({}))).toBeNull();
  });

  it('begins a form on the choice it would open on where the reader may give it, and on the lowest they may give where not', () => {
    expect(firstChoice(page({ choices: ['full_view', 'full_interact'] }), 'full_interact')).toBe('full_interact');
    expect(firstChoice(page({ choices: ['full_interact', 'full_manage'] }), 'full_view')).toBe('full_interact');
  });

  it('gives a role in a place only up to the reader’s own, as the role that goes with each choice', () => {
    const developer = page({ roles: ['viewer', 'interactor', 'developer'] });
    expect(mayGiveAsRole(developer, 'full_manage')).toBe(true);
    expect(highestRole(developer)).toBe('developer');
    const viewer = page({ roles: ['viewer'] });
    expect(mayGiveAsRole(viewer, 'full_view')).toBe(true);
    expect(mayGiveAsRole(viewer, 'full_interact')).toBe(false);
    expect(mayGiveChoice(page({ choices: ['full_view'] }), 'full_manage')).toBe(false);
  });

  it('tells a Developer of a place the role they hold, how far it goes and what is above it, and an Admin nothing', () => {
    const developer = page({ choices: ['full_view', 'full_interact', 'full_manage'], roles: ['viewer', 'interactor', 'developer'] });
    expect(whyNot(developer, words, { onPlace: true })).toBe('access.gives.youAre.developer {"place":"Web","choice":"full_manage","above":"roles.admin"}');
    const admin = page({ choices: ['full_view', 'full_interact', 'full_manage'], roles: ['viewer', 'interactor', 'developer', 'admin'] });
    expect(whyNot(admin, words, { onPlace: true })).toBeNull();
  });

  it('tells somebody who reads a thing that they share it to read, and somebody who takes part that managing it is not theirs', () => {
    expect(whyNot(page({ choices: ['full_view'] }), { ...words, kind: 'document' }, { onPlace: false })).toBe('access.gives.readOnly {"name":"Web","verb":"access.gives.verb.edit"}');
    expect(whyNot(page({ choices: ['full_view', 'full_interact'] }), { ...words, kind: 'thread' }, { onPlace: false })).toBe('access.gives.upTo {"name":"Web","choice":"full_interact"}');
    expect(whyNot(page({ choices: ['full_view', 'full_interact', 'full_manage'] }), words, { onPlace: false })).toBeNull();
  });

  it('knows a place that keeps sharing to owners only for a reader who gives nothing, and says who can', () => {
    expect(sharingIsKept(page({ holders_share: false }))).toBe(true);
    // An owner or a manager of the page is not shut out by the policy: their lists are not empty.
    expect(sharingIsKept(page({ holders_share: false, choices: ['full_view'] }))).toBe(false);
    // Somebody who holds nothing, where the place lets holders share, is not told that sharing is kept.
    expect(sharingIsKept(page({ holders_share: true }))).toBe(false);
    expect(keptLine(t, page({ holders_share: false }), 'Acme')).toBe('access.gives.kept {"place":"Acme","owners":"Ada Lovelace"}');
  });
});

describe('what sharing with agents comes to', () => {
  it('has the three areas on a thread, and on a place its chat and documents with, inside it, the conversations', () => {
    expect(areasOf('thread', 'inside')).toEqual(['timeline', 'chat', 'documents']);
    expect(areasOf('topic', 'inside')).toEqual(['timeline', 'chat', 'documents']);
    // A place shared by itself has no conversation of its own.
    expect(areasOf('topic', 'alone')).toEqual(['chat', 'documents']);
    expect(areasOf('chat', 'inside')).toEqual(['chat']);
    expect(areasOf('document', 'inside')).toEqual(['documents']);
  });

  it('makes Can view read in every area, Can edit write there and edit documents, and Can manage add the documents themselves and nothing for conversations or chats', () => {
    const areas = areasOf('topic', 'inside');
    expect(partsOfLevel('view', areas)).toEqual({ timeline: 'read', chat: 'read', documents: 'read' });
    expect(partsOfLevel('edit', areas)).toEqual({ timeline: 'write', chat: 'write', documents: 'edit' });
    expect(partsOfLevel('manage', areas)).toEqual({ timeline: 'write', chat: 'write', documents: 'manage' });
    expect(partsOfLevel('manage', ['chat'])).toEqual({ chat: 'write' });
  });

  it('names the everyday choice that parts amount to, and none for areas at different levels or an area left out', () => {
    const areas = areasOf('topic', 'inside');
    expect(levelOfParts(partsOfLevel('view', areas), areas)).toBe('view');
    expect(levelOfParts(partsOfLevel('edit', areas), areas)).toBe('edit');
    expect(levelOfParts(partsOfLevel('manage', areas), areas)).toBe('manage');
    expect(levelOfParts({ timeline: 'read', chat: 'write', documents: 'read' }, areas)).toBeNull();
    expect(levelOfParts({ documents: 'edit' }, areas)).toBeNull();
  });

  it('lets a reader share only what they may do themself: reading for a share to read, writing for one to write, and managing for one that manages', () => {
    const edit = { timeline: 'write', chat: 'write', documents: 'edit' } as const;
    expect(mayGiveParts(edit, ['view', 'edit'])).toBe(true);
    expect(mayGiveParts(edit, ['view'])).toBe(false);
    expect(mayGiveParts({ documents: 'manage' }, ['view', 'edit'])).toBe(false);
    expect(mayGiveParts({ documents: 'manage' }, ['view', 'edit', 'manage'])).toBe(true);
    // A share that names nothing needs nothing.
    expect(mayGiveParts({}, [])).toBe(true);
  });

  it('changes one area at a time and leaves the others as they were', () => {
    const before = { timeline: 'read', chat: 'read', documents: 'edit' } as const;
    const after = withStep(before, 'documents', 3);
    expect(after).toEqual({ timeline: 'read', chat: 'read', documents: 'manage' });
    expect(withStep(before, 'chat', 0)).toEqual({ timeline: 'read', documents: 'edit' });
    expect(stepOf('timeline', withStep(before, 'timeline', 2))).toBe(2);
    expect(sameParts(before, { timeline: 'read', chat: 'read', documents: 'edit' })).toBe(true);
  });

  it('gives a copy no write: whatever is shared is shared to read', () => {
    expect(readOnly({ timeline: 'write', chat: 'read', documents: 'manage' })).toEqual({ timeline: 'read', chat: 'read', documents: 'read' });
    expect(readOnly({ documents: 'edit' })).toEqual({ documents: 'read' });
  });
});

describe('letting people into the environment of a thread', () => {
  const environment = { id: '00000000-0000-4000-8024-000000000001', kind: 'environment', name: 'Main', owner: ADA };
  const dependency = (for_: string, permission: string, canGive: boolean): Dependency => ({ for: for_, permission, choice: for_ === 'thread.view' ? 'full_view' : null, object: environment, options: null, open: 'none', through: null, can_give: canGive });
  const threadPage = (gives: boolean[]): PermissionPage =>
    page({}, { object: { kind: 'thread', id: '00000000-0000-4000-8003-000000000004', name: 'Fix login' }, depends: [dependency('thread.view', 'environment.view', gives[0] ?? false), dependency('thread.manage', 'files.manage', gives[1] ?? false), dependency('thread.manage', 'terminal.manage', gives[2] ?? false)] });

  it('offers the environment’s Can view with any of the three choices, and with Can manage the Manage of Files and Terminal as well', () => {
    const view = offerFor(threadPage([true, true, true]), 'full_view');
    expect(view?.rows).toEqual(['environment.view', 'files.view', 'terminal.view', 'browser.view', 'vnc.view', 'ports.view']);
    expect(view?.giveManage).toEqual([]);
    const manage = offerFor(threadPage([true, true, true]), 'full_manage');
    expect(manage?.rows).toEqual(['environment.view', 'files.view', 'terminal.view', 'browser.view', 'vnc.view', 'ports.view', 'files.manage', 'terminal.manage']);
  });

  it('never offers the environment’s own Manage, nor the Manage of Browsers, VNC or Ports: those are given on its page, by name', () => {
    const rows = offerFor(threadPage([true, true, true]), 'full_manage')?.rows ?? [];
    for (const never of ['environment.manage', 'browser.manage', 'vnc.manage', 'ports.manage']) expect(rows).not.toContain(never);
  });

  it('offers nothing for a page that names no environment', () => {
    expect(offerFor(page({}), 'full_view')).toBeNull();
  });

  it('names the people on the environment’s page in one change: its Can view, and the Manage rows the person at the page may give', () => {
    const offer = offerFor(threadPage([true, true, false]), 'full_manage');
    if (offer === null) throw new Error('the page offers the environment');
    const holders = [{ kind: 'user' as const, id: ADA.id }];
    const change = letInChange(5, holders, offer, null);
    expect(change.expected_revision).toBe(5);
    expect(change.name).toEqual([{ choice: 'full_view', holders }]);
    // The terminal is not theirs to give: only Files is named in this change.
    expect(change.rows).toEqual([{ permission: 'files.manage', add: [{ holder: holders[0] }] }]);
    expect(offer.askManage).toEqual(['terminal.manage']);
  });

  it('asks the owner for what the person at the page may not give, for each person named and nobody else', () => {
    const offer = offerFor(threadPage([false, false, false]), 'full_view');
    if (offer === null) throw new Error('the page offers the environment');
    const body = askToLetIn(offer, [{ kind: 'user', id: ADA.id }, { kind: 'group', id: '00000000-0000-4000-8008-000000000001' }], 'Please', '00000000-0000-4000-8003-000000000004');
    expect(body?.items).toHaveLength(1);
    expect(body?.items[0]?.object).toEqual({ kind: 'environment', id: environment.id });
    expect(body?.items[0]?.holder).toEqual({ kind: 'user', id: ADA.id });
    expect(body?.items[0]?.permissions).toEqual(['environment.view', 'files.view', 'terminal.view', 'browser.view', 'vnc.view', 'ports.view']);
    expect(body?.reason).toBe('Please');
    // Where the person may give it all, there is nothing to ask.
    const given = offerFor(threadPage([true, true, true]), 'full_manage');
    if (given === null) throw new Error('the page offers the environment');
    expect(askToLetIn(given, [{ kind: 'user', id: ADA.id }], 'x', 'y')).toBeNull();
  });

  it('gives the door rows it holds while asking for the selected person’s remaining entry', () => {
    const offer = offerFor(threadPage([false, true, false]), 'full_manage');
    if (offer === null) throw new Error('the page offers the environment');
    const change = letInChange(5, [{ kind: 'user', id: ADA.id }], offer, null);
    expect(change.name).toBeUndefined();
    expect(change.rows?.map((row) => row.permission)).toEqual(['files.manage']);
    const asked = askToLetIn(offer, [{ kind: 'user', id: ADA.id }], 'Please', 'thread');
    expect(asked?.items[0]?.permissions).toContain('terminal.manage');
    expect(asked?.items[0]?.permissions).not.toContain('files.manage');
  });
});
