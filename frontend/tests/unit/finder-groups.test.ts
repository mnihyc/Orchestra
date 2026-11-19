// @vitest-environment node
/** What the finder lists: names as rows with the way to each, groups put in the order of how well they answer, and what a name leads to. */
import type { TFunction } from 'i18next';
import { Folder } from 'lucide-react';
import { describe, expect, it, vi } from 'vitest';

import { ancestorsOf, describe as describeName, destinationOf, pathOf, usernameOf, wayOf } from '@/features/finder/destinations';
import { matchScore } from '@/features/finder/ranking';
import { assemble, type Env, type FinderRow, rowOfName, type Scored } from '@/features/finder/rows';
import type { NameHit, NodeRef } from '@/generated/api-types';

/** The English words of the finder, as far as these tests read them. */
const WORDS: Record<string, string> = {
  'finder.kinds.org': 'org',
  'finder.kinds.department': 'department',
  'finder.kinds.project': 'project',
  'finder.kinds.folder': 'folder',
  'finder.kinds.topic': 'topic',
  'finder.kinds.thread': 'thread',
  'finder.kinds.user': 'person',
  'finder.kinds.group': 'group',
  'finder.kinds.provider': 'provider',
  'finder.private': 'Private {{kind}}',
  'finder.groups.go': 'Go to',
  'finder.groups.commands': 'Commands',
  'finder.groups.settings': 'Settings',
  'finder.groups.help': 'Help',
  'finder.groups.recent': 'Recent',
};
const t = ((key: string, values?: Record<string, string>) => (WORDS[key] ?? key).replace('{{kind}}', values?.kind ?? '')) as unknown as TFunction;

const ref = (id: string, kind: NodeRef['kind'], name: string | null, redacted = false): NodeRef => ({ id, kind, name, redacted });
const hit = (more: Partial<NameHit> & Pick<NameHit, 'kind' | 'id' | 'name'>): NameHit => ({ href: null, tag: null, thread_attention: null, color: null, custom_color: null, address: null, username: null, avatar: null, path: [], owner: null, archived: false, recent: false, ...more });

const env = (more: Partial<Env> = {}): Env => ({ t, go: vi.fn(), orgSlug: 'acme', exists: () => true, showCard: vi.fn(), ...more });

describe('where a name lies', () => {
  it('keeps the server’s own icon colour even without a held tree', () => {
    const row = rowOfName(hit({ kind: 'project', id: 'remote', name: 'Remote', custom_color: '#3a7bd5', color: null }), 'go', 'remote', env());
    expect(row.place).toEqual({ kind: 'project', id: 'remote', color: null, own: '#3a7bd5' });
  });
  const acme = ref('o', 'org', 'Acme');
  const web = ref('p', 'project', 'Web');

  it('says the way down from the root org, without the thing itself, whether or not the path ends with it', () => {
    const topic = hit({ kind: 'topic', id: 't', name: 'Fix login', path: [acme, web] });
    const withSelf = hit({ kind: 'topic', id: 't', name: 'Fix login', path: [acme, web, ref('t', 'topic', 'Fix login')] });
    expect(wayOf(topic, t)).toBe('Acme › Web');
    expect(wayOf(withSelf, t)).toBe('Acme › Web');
    expect(ancestorsOf(withSelf).map((place) => place.name)).toEqual(['Acme', 'Web']);
  });

  it('calls a project that lies in a project a folder, and lists it with the way to it', () => {
    const folder = hit({ kind: 'project', id: 'c', name: 'Checkout', path: [acme, web, ref('c', 'project', 'Checkout')] });
    expect(describeName(folder)).toBe('folder');
    expect(wayOf(folder, t)).toBe('Acme › Web');
    const row = rowOfName(folder, 'go', 'check', env());
    expect(row.title).toBe('Checkout');
    expect(row.detail).toBe('folder: Acme › Web');
    // A folder deeper still is told the same, with its whole way.
    const deeper = hit({ kind: 'project', id: 'd', name: 'Payments', path: [acme, web, ref('c', 'project', 'Checkout'), ref('d', 'project', 'Payments')] });
    expect(describeName(deeper)).toBe('folder');
    expect(wayOf(deeper, t)).toBe('Acme › Web › Checkout');
  });

  it('says which place a row stands for, so that it is drawn with the icon of its kind in the colour the place was given, and says none of what is no place', () => {
    const place = (more: Partial<NameHit> & Pick<NameHit, 'kind' | 'id' | 'name'>) => rowOfName(hit(more), 'go', 'x', env()).place;
    expect(place({ kind: 'project', id: 'p', name: 'Web', path: [acme, ref('p', 'project', 'Web')] })).toEqual({ kind: 'project', id: 'p', color:null, own:null });
    expect(place({ kind: 'project', id: 'c', name: 'Checkout', path: [acme, web, ref('c', 'project', 'Checkout')] })).toEqual({ kind: 'folder', id: 'c', color:null, own:null });
    expect(place({ kind: 'org', id: 'r', name: 'Research', path: [acme, ref('r', 'org', 'Research')] })).toEqual({ kind: 'org', id: 'r', color:null, own:null });
    expect(place({ kind: 'topic', id: 't', name: 'Fix login', path: [acme, web] })).toEqual({ kind: 'topic', id: 't', color:null, own:null });
    expect(place({ kind: 'thread', id: 'h', name: 'Main thread', path: [acme, web] })).toEqual({ kind: 'thread', id: 'h', color:null, own:null });
    expect(place({ kind: 'document', id: 'd', name: 'Plan', path: [acme, web] })).toEqual({ kind: 'document', id: 'd', color:null, own:null });
    expect(place({ kind: 'user', id: 'u', name: 'Dora', address: 'acme.dora' })).toBeUndefined();
    expect(place({ kind: 'provider', id: 'v', name: 'Team Anthropic' })).toBeUndefined();
  });

  it('keeps calling a project in an org a project, and an org in an org a department', () => {
    expect(describeName(hit({ kind: 'project', id: 'p', name: 'Web', path: [acme, ref('p', 'project', 'Web')] }))).toBe('project');
    expect(describeName(hit({ kind: 'org', id: 'r', name: 'Research', path: [acme, ref('r', 'org', 'Research')] }))).toBe('department');
    expect(describeName(hit({ kind: 'org', id: 'o', name: 'Acme', path: [acme] }))).toBe('org');
  });

  it('never names a place the reader may not know: it is "Private project"', () => {
    const lying = hit({ kind: 'topic', id: 't', name: 'Budget', path: [acme, ref('x', 'project', null, true)] });
    expect(wayOf(lying, t)).toBe('Acme › Private project');
  });

  it('keeps a person’s name and handle readable while still finding the username from a legacy address', () => {
    const person = hit({ kind: 'user', id: 'u', name: 'Dora Developer', address: 'acme.research.dora' });
    const row = rowOfName(person, 'people', 'dora', env());
    expect(row.title).toBe('Dora Developer');
    expect(row.detail).toBe('person: @dora');
    expect(usernameOf(person)).toBe('dora');
  });

  it.each(['recent', 'starred', 'people', 'go', 'resources'] as const)('keeps routing addresses out of %s context without changing destinations', (group) => {
    const person = hit({ kind: 'user', id: 'u', name: 'Dora Developer', username: 'dora', address: 'acme.old-dora', href: '/u/dora' });
    expect(rowOfName(person, group, '', env()).detail).toBe('person: @dora');
    expect(rowOfName(hit({ kind: 'org', id: 'o', name: 'Acme', address: 'acme', path: [acme] }), group, '', env()).detail).toBe('org');
    expect(rowOfName(hit({ kind: 'group', id: 'crew', name: 'Crew', address: 'acme.crew', path: [acme] }), group, '', env()).detail).toBe('group: Acme');
    const e = env();
    const row = rowOfName(person, group, '', e);
    row.run('here');
    expect(e.go).toHaveBeenCalledExactlyOnceWith({ to: '/u/dora' }, 'here');
  });

  it('uses the explicit username for words and the served address for opening a person', () => {
    const person = hit({kind:'user',id:'u',name:'Dora',address:'acme.old-address',username:'dora',href:'/u/dora'});
    const e = env();
    const row = rowOfName(person,'people','dora',e);
    expect(usernameOf(person)).toBe('dora');
    row.run('here');
    expect(e.showCard).not.toHaveBeenCalled();
    expect(e.go).toHaveBeenCalledWith({to:'/u/dora'},'here');
    expect(row.keeps).toBeUndefined();
    row.run('tab');
    expect(e.go).toHaveBeenCalledWith({to:'/u/dora'},'tab');
  });
});

describe('where a name leads', () => {
  it('opens the served address even where it disagrees with a name, address or held route template', () => {
    for (const [kind, href] of [
      ['org', '/org/stable-org-id'], ['mcp_server', '/resources/mcp/server'], ['mount', '/resources/mounts/data'],
      ['user', '/u/%E6%9E%97'], ['chat', '/org/its-own-org/messages?chat=direct'],
    ] as const) {
      const named = hit({ kind, id: 'x1', name: 'Old name', address: 'old.address', href });
      const destination = destinationOf(named);
      expect(destination).toEqual({ to: href });
      expect(pathOf(destination ?? { to: '' })).toBe(href);
      const e = env({ orgSlug: 'a-different-org', exists: () => false });
      rowOfName(named, 'go', '', e).run('here');
      expect(e.go).toHaveBeenCalledWith({ to: href }, 'here');
      expect(e.showCard).not.toHaveBeenCalled();
    }
    expect(destinationOf(hit({ kind: 'group', id: 'g', name: 'People' }))).toBeNull();
  });

  it('writes an address with its segments escaped and its search after a question mark', () => {
    expect(pathOf({ to: '/org/$slug', params: { slug: 'a b/c' } })).toBe('/org/a%20b%2Fc');
    expect(pathOf({ to: '/thread/$threadId', params: { threadId: 'x' }, search: { item: 'i-1', q: undefined } })).toBe('/thread/x?item=i-1');
    expect(pathOf({ to: '/topic/t?chat=c#where', search: { panel: 'chat' } })).toBe('/topic/t?chat=c&panel=chat#where');
  });

  it('opens a name where Enter would, but shows the card of one that has no screen', () => {
    const e = env({ exists: (to) => to !== '/u/$username' });
    const row = rowOfName(hit({ kind: 'user', id: 'u', name: 'Dora', address: 'acme.dora' }), 'people', '', e);
    row.run('here');
    expect(e.showCard).toHaveBeenCalledTimes(1);
    expect(e.go).not.toHaveBeenCalled();
    expect(row.keeps).toBe(true);
    expect(row.destination).toBeUndefined();

    const topic = rowOfName(hit({ kind: 'topic', id: 't', name: 'Fix login', href: '/topic/t' }), 'go', '', e);
    topic.run('tab');
    expect(e.go).toHaveBeenCalledWith({ to: '/topic/t' }, 'tab');
    expect(topic.destination).toEqual({ to: '/topic/t' });
  });
});

describe('the groups of the finder', () => {
  const row = (id: string, group: FinderRow['group']): FinderRow => ({ id, group, title: id, marks: [], detail: '', icon: Folder, run: () => undefined });
  const scored = (group: FinderRow['group'], scores: number[]): Scored[] => scores.map((score, index) => ({ score, row: row(`${group}${index}`, group) }));

  it('puts the group that answers best first, whatever kind it is', () => {
    const groups = assemble({
      mode: 'all',
      empty: false,
      answers: { go: scored('go', [0.6]), commands: scored('commands', [0.9]), settings: scored('settings', [1.25, 0.4]) },
      expanded: new Set(),
      t,
    });
    expect(groups.map((group) => group.id)).toEqual(['settings', 'commands', 'go']);
  });

  it('keeps the order of the list for groups that answer alike, and for nothing typed', () => {
    const answers = { help: scored('help', [0.5]), go: scored('go', [0.5]), recent: scored('recent', [0.5]) };
    expect(assemble({ mode: 'all', empty: false, answers, expanded: new Set(), t }).map((group) => group.id)).toEqual(['recent', 'go', 'help']);
    expect(assemble({ mode: 'all', empty: true, answers: { help: scored('help', [9]), recent: scored('recent', [0]) }, expanded: new Set(), t }).map((group) => group.id)).toEqual(['recent', 'help']);
  });

  it('cuts a group to what a glance holds and says how many more it has, until it is asked to show them', () => {
    const answers = { go: scored('go', Array.from({ length: 15 }, (_, index) => 1 - index / 100)), commands: scored('commands', [0.3]) };
    const cut = assemble({ mode: 'all', empty: false, answers, expanded: new Set(), t });
    const go = cut.find((group) => group.id === 'go');
    expect(go?.rows).toHaveLength(8);
    expect(go?.more).toBe(7);
    const shown = assemble({ mode: 'all', empty: false, answers, expanded: new Set(['go']), t }).find((group) => group.id === 'go');
    expect(shown?.rows).toHaveLength(15);
    expect(shown?.more).toBe(0);
  });

  it('shows more of a group that stands alone, which a sign keeps it to', () => {
    const answers = { commands: scored('commands', Array.from({ length: 12 }, () => 0.5)), go: scored('go', [1]) };
    const commands = assemble({ mode: 'commands', empty: false, answers, expanded: new Set(), t });
    expect(commands.map((group) => group.id)).toEqual(['commands']);
    expect(commands[0]?.rows).toHaveLength(12);
  });

  it('leaves out a group with nothing in it, and a mode lists only its own groups', () => {
    expect(assemble({ mode: 'all', empty: false, answers: { go: [], help: scored('help', [1]) }, expanded: new Set(), t }).map((group) => group.id)).toEqual(['help']);
    expect(assemble({ mode: 'people', empty: false, answers: { go: scored('go', [1]), people: scored('people', [1]) }, expanded: new Set(), t }).map((group) => group.id)).toEqual(['people']);
    expect(assemble({ mode: 'settings', empty: false, answers: { rows: scored('rows', [0.4]), settings: scored('settings', [0.8]) }, expanded: new Set(), t }).map((group) => group.id)).toEqual(['settings', 'rows']);
  });
});

describe('how a name that the server found answers the letters', () => {
  it('scores the name itself, its beginning, a word of it, and the inside of it', () => {
    expect(matchScore('Fix', 'fix')).toBeGreaterThan(matchScore('Fix login', 'fix'));
    expect(matchScore('Fix login', 'fix')).toBeGreaterThan(matchScore('Hot fix', 'fix'));
    expect(matchScore('Hot fix', 'fix')).toBeGreaterThan(matchScore('Prefixed', 'fix'));
    expect(matchScore('Prefixed', 'fix')).toBeGreaterThan(matchScore('Dora', 'fix'));
  });
});
