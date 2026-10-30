/** What the settings screens decide without drawing anything: where a value in force comes from, which control a key's schema asks for, what a kind of channel's form sends, and the small rules of the personal pages. */
import { describe, expect, it } from 'vitest';

import { testNeedsAddress } from '@/features/settings/org/Channels';
import { ruleSources } from '@/features/settings/org/Invite';
import { liesIn, orgPlaces } from '@/features/settings/org/MoveMember';
import { configOf, fieldsForEdit, missingFields, secretsOf, startingValues } from '@/features/settings/notifierFields';
import { emptied, helpOf, keptOnDevice, kindOfKept, knownKept, namesIn, withoutName } from '@/features/settings/personal/Interface';
import { browserName } from '@/features/settings/personal/Security';
import { endOfToken, permissionsByArea } from '@/features/settings/personal/Tokens';
import { bytesOf, byGroup, groupOf, honoursLocks, keysAt, lockAbove, provenanceOf, sameValue, shapeOf, sizeParts, withGroup } from '@/features/settings/settingsModel';
import type { Field, PermissionDef, ResolvedSetting, SettingDef, TreeNode } from '@/generated/api-types';

const def = (key: string, more: Partial<SettingDef> = {}): SettingDef => ({
  key,
  title: key,
  description: '',
  group: '',
  schema: { type: 'boolean' },
  class: 'policy',
  scopes: ['org', 'project'],
  merge: 'replace',
  default: null,
  example: false,
  text_key: null,
  keywords: [],
  ...more,
});

const setting = (more: Partial<ResolvedSetting> = {}): ResolvedSetting => ({
  key: 'k',
  value: 1,
  source: { scope: 'default', node: null },
  own: null,
  locked_by: null,
  writable: true,
  parts: [],
  ...more,
});

const ACME = { id: 'acme', kind: 'org', name: 'Acme', redacted: false } as const;

describe('where the value in force comes from', () => {
  const here = { kind: 'node', id: 'research' } as const;
  const SOMEWHERE = { id: 'research', kind: 'org', name: 'Research', redacted: false } as const;

  it('is the default while nothing was set anywhere', () => {
    expect(provenanceOf(setting(), here)).toEqual({ kind: 'default', scope: 'default' });
  });

  it('is the place above that set it, by name', () => {
    expect(provenanceOf(setting({ source: { scope: 'org', node: ACME } }), here)).toEqual({ kind: 'inherited', scope: 'org', place: 'Acme' });
  });

  it('is here once this place set it, whatever is above', () => {
    expect(provenanceOf(setting({ source: { scope: 'project', node: { ...ACME, kind: 'project' } }, own: { value: 3, locked: false, revision: 2 } }), here)).toEqual({ kind: 'here', locked: false });
  });

  it('is a lock when a place above does not let it be changed, and says where, even over a value set here', () => {
    const locked = setting({ locked_by: { scope: 'org', node: ACME }, own: { value: 3, locked: false, revision: 1 } });
    expect(provenanceOf(locked, here)).toEqual({ kind: 'locked', scope: 'org', place: 'Acme' });
    expect(lockAbove(locked, here)).toEqual({ scope: 'org', node: ACME });
  });

  it('is here, and no lock from above, when the lock named is the row of the place itself', () => {
    const mine = setting({ source: { scope: 'org', node: SOMEWHERE }, locked_by: { scope: 'org', node: SOMEWHERE }, own: { value: false, locked: true, revision: 4 } });
    expect(lockAbove(mine, here)).toBeNull();
    expect(provenanceOf(mine, here)).toEqual({ kind: 'here', locked: true });
    // The same row read from a place below is a lock from above.
    expect(provenanceOf({ ...mine, own: null }, { kind: 'node', id: 'web' })).toEqual({ kind: 'locked', scope: 'org', place: 'Research' });
  });

  it('is a lock from above for the own value of an environment or a provider, and none for the system', () => {
    const named = setting({ locked_by: { scope: 'org', node: SOMEWHERE } });
    expect(lockAbove(named, { kind: 'environment', id: 'env' })).not.toBeNull();
    expect(lockAbove(named, { kind: 'provider', id: 'prov' })).not.toBeNull();
    expect(lockAbove(named, { kind: 'system' })).toBeNull();
  });
});

describe('the keys of a place', () => {
  it('are those valid at its scope, shown in their groups in the order the screen has them', () => {
    const defs = [
      withGroup({ ...def('terminal.enabled'), group: 'shells' }),
      withGroup({ ...def('agent.model'), group: 'agent' }),
      withGroup({ ...def('thread.cwd', { scopes: ['thread'] }), group: 'environment' }),
    ];
    expect(keysAt(defs, 'org').map((entry) => entry.key)).toEqual(['terminal.enabled', 'agent.model']);
    expect(byGroup(keysAt(defs, 'org')).map((entry) => entry.group)).toEqual(['agent', 'shells']);
  });

  it('are put in a group by their name when the registry names none', () => {
    expect(groupOf(def('invitations.max_uses'))).toBe('joining');
    expect(groupOf(def('docs.max_bytes'))).toBe('chat_documents');
    expect(groupOf(def('something.unheard_of'))).toBe('system');
    expect(groupOf({ ...def('agent.model'), group: 'models' })).toBe('models');
  });

  it('can be locked only where a lock changes what lower places can do', () => {
    expect(honoursLocks({ merge: 'replace' })).toBe(true);
    expect(honoursLocks({ merge: 'map' })).toBe(true);
    expect(honoursLocks({ merge: 'intersect' })).toBe(false);
    expect(honoursLocks({ merge: 'min' })).toBe(false);
  });
});

describe('the control a schema asks for', () => {
  it('is read from the JSON Schema of the registry', () => {
    expect(shapeOf({ type: 'boolean' })).toEqual({ type: 'bool' });
    expect(shapeOf({ type: 'integer', minimum: 1, maximum: 100, 'x-unit': 'minutes' })).toEqual({ type: 'int', min: 1, max: 100, unit: 'minutes' });
    expect(shapeOf({ type: 'string', enum: ['ask', 'auto'] })).toEqual({ type: 'choice', values: ['ask', 'auto'] });
    expect(shapeOf({ type: 'string', format: 'uuid' })).toEqual({ type: 'id' });
    expect(shapeOf({ type: 'array', items: { type: 'string', enum: ['a', 'b'] }, uniqueItems: true })).toEqual({ type: 'list', item: { type: 'choice', values: ['a', 'b'] }, unique: true });
    expect(shapeOf({ type: 'object', additionalProperties: { type: 'boolean' } })).toEqual({ type: 'map', value: { type: 'bool' }, byId: false });
    expect(shapeOf({ type: 'object', additionalProperties: { type: 'string' }, propertyNames: { format: 'uuid' } })).toEqual({ type: 'map', value: { type: 'text', maxLength: null, pattern: null }, byId: true });
  });

  it('is an object of fields where it has members, and a value that may be nothing where it is one or null', () => {
    const triggers = shapeOf({ type: 'object', properties: { on: { type: 'array', items: { type: 'string' } }, min_interval_secs: { type: 'integer', minimum: 0 } }, required: ['on'], additionalProperties: false });
    expect(triggers).toMatchObject({ type: 'object', open: false });
    expect(triggers.type === 'object' ? triggers.members.map((member) => [member.name, member.required]) : []).toEqual([
      ['on', true],
      ['min_interval_secs', false],
    ]);
    expect(shapeOf({ anyOf: [{ type: 'string' }, { type: 'null' }] })).toMatchObject({ type: 'nullable' });
  });

  it('is JSON for what it cannot draw, so that no key is left without a control', () => {
    expect(shapeOf({})).toEqual({ type: 'json' });
    expect(shapeOf({ anyOf: [{ type: 'boolean' }, { type: 'string' }] })).toEqual({ type: 'json' });
    expect(shapeOf(null)).toEqual({ type: 'json' });
  });

  it('compares values as JSON, whatever order an object came in', () => {
    expect(sameValue({ a: 1, b: [1, 2] }, { b: [1, 2], a: 1 })).toBe(true);
    expect(sameValue([1, 2], [2, 1])).toBe(false);
    expect(sameValue({ a: 1 }, { a: 1, b: undefined })).toBe(true);
  });

  it('shows a number of bytes in the largest size it is a whole number of, and back', () => {
    expect(sizeParts(67_108_864)).toEqual({ amount: 64, unit: 'MiB' });
    expect(sizeParts(1_500)).toEqual({ amount: 1_500, unit: 'B' });
    expect(sizeParts(0)).toEqual({ amount: 0, unit: 'B' });
    expect(bytesOf(64, 'MiB')).toBe(67_108_864);
  });
});

describe('what a kind of channel sends', () => {
  const fields: Field[] = [
    { name: 'url', label: 'Address', type: 'text', required: true, help: null, default: null, choices: null, max_bytes: null },
    { name: 'secret', label: 'Secret', type: 'secret', required: false, help: null, default: null, choices: null, max_bytes: null },
    { name: 'notify_all', label: 'Notify all', type: 'boolean', required: false, help: null, default: null, choices: null, max_bytes: null },
    { name: 'headers', label: 'Headers', type: 'map', required: false, help: 'Sent with each call.', default: null, choices: null, max_bytes: null },
  ];

  it('keeps secrets out of the config, sends only the secrets that were typed, and leaves out what is empty', () => {
    const values = { url: ' https://hooks.example/x ', secret: 'abc', notify_all: true, headers: {} };
    expect(configOf(fields, values)).toEqual({ url: 'https://hooks.example/x', notify_all: true });
    expect(secretsOf(fields, values)).toEqual({ secret: 'abc' });
    expect(secretsOf(fields, { ...values, secret: '' })).toEqual({});
  });

  it('starts from what the channel holds, with every secret empty', () => {
    expect(startingValues(fields, { url: 'https://x', notify_all: true, headers: { a: 'b' }, secret: 'ignored' })).toEqual({ url: 'https://x', secret: '', notify_all: true, headers: { a: 'b' } });
  });

  it('asks for the required fields, and counts a secret that is set already as given', () => {
    const secretNeeded: Field[] = [{ ...fields[1], required: true } as Field];
    expect(missingFields(fields, { url: '', secret: '', notify_all: false, headers: {} })).toEqual(['url']);
    expect(missingFields(secretNeeded, { secret: '' })).toEqual(['secret']);
    expect(missingFields(secretNeeded, { secret: '' }, ['secret'])).toEqual([]);
    expect(fieldsForEdit(secretNeeded, ['secret'], 'Leave empty to keep it.')[0]).toMatchObject({ required: false, help: 'Leave empty to keep it.' });
  });

  it('is tried with an address that is asked for only where it reaches people alone and is not linked by a chat', () => {
    expect(testNeedsAddress({ links: false, targets: ['person'] })).toBe(true);
    expect(testNeedsAddress({ links: true, targets: ['person'] })).toBe(false);
    expect(testNeedsAddress({ links: false, targets: ['person', 'destination'] })).toBe(false);
  });
});

describe('the rules an org gives its invitations', () => {
  it('say which places set them, once each, and none for what is shipped', () => {
    const rules = [
      setting({ key: 'invitations.forms', source: { scope: 'org', node: ACME } }),
      setting({ key: 'invitations.max_hours', source: { scope: 'org', node: ACME } }),
      setting({ key: 'invitations.max_uses', source: { scope: 'default', node: null } }),
    ];
    expect(ruleSources(rules)).toEqual(['Acme']);
    expect(ruleSources([setting({ key: 'invitations.forms' })])).toEqual([]);
  });
});

describe('moving a member', () => {
  const rows = new Map<string, TreeNode>(
    [
      ['acme', 'Acme', null],
      ['research', 'Research', 'acme'],
      ['ml', 'ML', 'research'],
      ['ops', 'Ops', 'acme'],
    ].map(([id, name, parent]): [string, TreeNode] => [
      id ?? '',
      { id: id ?? '', kind: 'org', name: name ?? '', address: null, parent_id: parent ?? null, archived: false, color: null, deleted: false, docs: 0, restricted: false, participating: true, chat: { id: id ?? '', seq: 0 }, thread: null, topic: null, tree_seq: 1 },
    ]),
  );

  it('offers the orgs of the tree by the names they lie under, in the order they are read', () => {
    expect(orgPlaces(rows, ' / ').map((place) => place.path)).toEqual(['Acme', 'Acme / Ops', 'Acme / Research', 'Acme / Research / ML']);
  });

  it('knows an org that lies in another, so that moving an admin up says they will be an admin of all of it', () => {
    const places = orgPlaces(rows, ' / ');
    expect(liesIn(places, 'ml', 'acme')).toBe(true);
    expect(liesIn(places, 'ml', 'research')).toBe(true);
    expect(liesIn(places, 'research', 'ml')).toBe(false);
    expect(liesIn(places, 'ops', 'research')).toBe(false);
  });
});

describe('a token', () => {
  it('ends at the end of the day that was chosen, or after the days that were, or never', () => {
    const now = Date.parse('2026-10-12T10:00:00.000Z');
    expect(endOfToken('never', '', now)).toBeNull();
    expect(endOfToken('30', '', now)).toBe('2026-11-11T10:00:00.000Z');
    expect(endOfToken('date', '2027-01-31', now)).toBe('2027-01-31T23:59:59.000Z');
    expect(endOfToken('date', '', now)).toBeNull();
  });

  it('is given a choice of permissions by area, in the order of the catalogue', () => {
    const permission = (name: string, area: string): PermissionDef => ({ name, area, label: name, allows: '', level: 'view', on: [], holders: [], options_schema: null, profiles: false, with: null });
    expect(permissionsByArea([permission('a', 'Threads'), permission('b', 'Topics'), permission('c', 'Threads')]).map((entry) => [entry.area, entry.items.map((item) => item.name)])).toEqual([
      ['Threads', ['a', 'c']],
      ['Topics', ['b']],
    ]);
  });
});

describe('a session', () => {
  it('names its browser as a person would, and gives the line itself where it is none we know', () => {
    expect(browserName('Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36')).toBe('Chrome (Linux)');
    expect(browserName('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 Version/17.0 Safari/605.1.15')).toBe('Safari (macOS)');
    expect(browserName('Mozilla/5.0 (Windows NT 10.0; Win64; x64) Gecko/20100101 Firefox/131.0')).toBe('Firefox (Windows)');
    expect(browserName('curl/8.14.1')).toBe('curl/8.14.1');
  });
});

describe('what the interface keeps for a person', () => {
  it('reads names out of a list, a list of entries with a name, or a map, and writes back in the shape it found', () => {
    expect(namesIn(['a', 'b'])).toEqual(['a', 'b']);
    expect(namesIn([{ name: 'Wide', layout: 1 }, { name: 'Narrow' }])).toEqual(['Wide', 'Narrow']);
    expect(namesIn({ Wide: {}, Narrow: {} })).toEqual(['Wide', 'Narrow']);
    expect(withoutName({ Wide: 1, Narrow: 2 }, 'Wide')).toEqual({ Narrow: 2 });
    expect(withoutName([{ name: 'Wide' }, { name: 'Narrow' }], 'Wide')).toEqual([{ name: 'Narrow' }]);
    expect(emptied(['a'])).toEqual([]);
    expect(emptied({ a: 1 })).toEqual({});
  });

  it('names what the application keeps on a device in words, and shows a name it does not know as it is', () => {
    expect(knownKept('recents')).toBe('recents');
    expect(knownKept('tree.00000000-0000-4000-8002-000000000064')).toBe('tree');
    expect(knownKept('archived.x')).toBe('archived');
    expect(knownKept('something-else')).toBeNull();
  });

  it('lists the tutorials finished and the hints closed from the help it holds, and nothing from what it does not understand', () => {
    expect(helpOf({ help: { finished: ['first-thread'], closed: ['x', 'y'] } })).toEqual({ tutorials: ['first-thread'], hints: ['x', 'y'] });
    expect(helpOf({ help: 'nonsense' })).toEqual({ tutorials: [], hints: [] });
    expect(helpOf(undefined)).toEqual({ tutorials: [], hints: [] });
  });

  it('lists what a device keeps for one person and never for another', () => {
    const held = new Map([
      ['orch.ada.workbench.thread', '{"layout":1}'],
      ['orch.ada.org', 'acme'],
      ['orch.grace.workbench.thread', 'x'],
      ['orch.appearance', '{}'],
    ]);
    const storage = { length: held.size, key: (at: number) => [...held.keys()][at] ?? null, getItem: (key: string) => held.get(key) ?? null };
    expect(keptOnDevice('ada', storage).map((item) => item.key)).toEqual(['orch.ada.org', 'orch.ada.workbench.thread']);
    expect(kindOfKept('ada', 'orch.ada.workbench.thread')).toEqual({ kind: 'layout', screen: 'thread' });
    expect(kindOfKept('ada', 'orch.ada.org')).toEqual({ kind: 'other', name: 'org' });
  });
});
