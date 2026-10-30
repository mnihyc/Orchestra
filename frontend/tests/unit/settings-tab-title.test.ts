/** Section identity uses only the current authorized path and existing shortest-place labels. */
import { expect, it } from 'vitest';
import type { NodeRef } from '@/generated/api-types';
import { knownPlaces, type Placed } from '@/lib/placeLabel';
import { settingsTabTitle, type SettingsTitlePlace } from '@/features/workbench/settingsTabTitle';

const ref = (id: string, name: string | null, redacted = false): NodeRef => ({ id, kind: 'org', name, redacted });
const at = (id: string, name: string, ...above: [string, string | null][]): Placed => ({ id, name, above: above.map(([key, text]) => ({ id: key, name: text })) });
const source = (id: string, name: string, ...above: [string, string | null][]): SettingsTitlePlace => ({ id, path: [...above.map(([key, text]) => ref(key, text, text === null)), ref(id, name)] });

it.each([['General', 'Defaults'], ['常规', '默认设置']] as const)('retains the section and readable place independently of locale (%s)', (general, defaults) => {
  const node = source('web', 'Web', ['acme', 'Acme']);
  expect(settingsTabTitle(general, node, knownPlaces([]))).toBe(`${general} · Web`);
  expect(settingsTabTitle(defaults, node, knownPlaces([]))).toBe(`${defaults} · Web`);
});

it('uses the shortest existing org qualifier when distinct places share their name', () => {
  const known = knownPlaces([at('web-a', 'Web', ['a', 'Acme'], ['ra', 'Research']), at('web-b', 'Web', ['b', 'Beta'], ['rb', 'Research'])]);
  expect(settingsTabTitle('General', source('web-a', 'Web', ['a', 'Acme'], ['ra', 'Research']), known)).toBe('General · Web · Acme');
  expect(settingsTabTitle('General', source('web-b', 'Web', ['b', 'Beta'], ['rb', 'Research']), known)).toBe('General · Web · Beta');
});

it('uses a nearer parent when the shared root does not distinguish equal names', () => {
  const known = knownPlaces([at('web-a', 'Web', ['a', 'Acme'], ['r', 'Research']), at('web-b', 'Web', ['a', 'Acme'], ['o', 'Operations'])]);
  expect(settingsTabTitle('General', source('web-a', 'Web', ['a', 'Acme'], ['r', 'Research']), known)).toBe('General · Web · Research');
  expect(settingsTabTitle('Audit', source('web-b', 'Web', ['a', 'Acme'], ['o', 'Operations']), known)).toBe('Audit · Web · Operations');
});

it('uses a readable native address for same-named root orgs without inventing an ID suffix', () => {
  const known = knownPlaces([{ id: 'a', name: 'Acme', above: [], address: 'acme' }, { id: 'b', name: 'Acme', above: [], address: 'acme-eu' }]);
  expect(settingsTabTitle('General', { ...source('a', 'Acme'), address: 'acme' }, known)).toBe('General · Acme · acme');
  expect(settingsTabTitle('General', { ...source('b', 'Acme'), address: 'acme-eu' }, known)).toBe('General · Acme · acme-eu');
});

it('does not distinguish duplicate instances of the same resource with arbitrary numbers', () => {
  const node = source('web', 'Web', ['acme', 'Acme']);
  const known = knownPlaces([at('web', 'Web', ['acme', 'Acme']), at('web', 'Web', ['acme', 'Acme'])]);
  expect(settingsTabTitle('General', node, known)).toBe('General · Web');
  expect(settingsTabTitle('General', { ...node }, known)).toBe('General · Web');
});

it('never fills redacted ancestors from a cached copy of that same source ID', () => {
  const known = knownPlaces([at('web', 'Web', ['secret-root', 'Private root'], ['secret-dept', 'Private department']), at('other', 'Web', ['beta', 'Beta'], ['open', 'Operations'])]);
  const node: SettingsTitlePlace = { id: 'web', path: [ref('secret-root', 'Private root', true), ref('secret-dept', 'Private department', true), ref('web', 'Web')] };
  const title = settingsTabTitle('General', node, known);
  expect(title).toBe('General · Web');
  expect(title).not.toMatch(/Private|secret|Operations|Beta/);
});

it('does not label a redacted self even when the cache remembers its exact private name and address', () => {
  const node: SettingsTitlePlace = { id: 'private', path: [ref('private', 'Retained private name', true)], address: 'private-address' };
  const known = knownPlaces([{ id: 'private', name: 'Retained private name', above: [], address: 'private-address' }]);
  expect(settingsTabTitle('General', node, known)).toBe('General');
});

it('does not borrow another path or infer a name from the requested ID', () => {
  const known = knownPlaces([at('requested', 'Cached private name')]);
  expect(settingsTabTitle('General', { id: 'requested', path: [ref('different', 'Wrong source')] }, known)).toBe('General');
  expect(settingsTabTitle('General', { id: 'requested', path: [] }, known)).toBe('General');
  expect(settingsTabTitle('General', undefined, known)).toBe('General');
});

it('uses the current source name over an older authorized tree row with the same identity', () => {
  const known = knownPlaces([at('web', 'Old name', ['acme', 'Acme'])]);
  expect(settingsTabTitle('General', source('web', 'New name', ['acme', 'Acme']), known)).toBe('General · New name');
});
