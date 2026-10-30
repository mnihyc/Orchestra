/** What every resources screen says once for all: the acts a row has and the menu and keys they make, the list that the way back returns to as it was left, the words that every hint has in both languages, and when a machine is to be enrolled again. */
import { describe, expect, it, vi } from 'vitest';
import { createInstance } from 'i18next';

import { type Act, entriesOf, entryOf, keysOf } from '@/features/resources/acts';
import { HINTED_FORMS, hasSettingHint, SETTING_HINTS } from '@/features/resources/hints';
import { settingDefinitionForKey, settingHelpEntries } from '@/features/help/settings';
import { shortRoot } from '@/features/resources/environments/words';
import { lostEnrollment, registeredCount } from '@/features/resources/machines/EnrollAgain';
import { listAsLeft, type ResourceKind, rememberList } from '@/features/resources/trail';
import type { Machine } from '@/generated/api-types';
import english from '@/i18n/en.json';
import chinese from '@/i18n/zh-CN.json';

const act = (id: string, extra: Partial<Act> = {}): Act => ({ id, label: id, run: vi.fn(), ...extra });

describe('the acts of a row', () => {
  it('make the entries of one menu, a line between the groups, and nothing for an act that is not there or a group that is empty', () => {
    const entries = entriesOf([act('open')], [null, false], [act('edit'), null, act('rename')], [act('delete', { danger: true })]);
    expect(entries.map((entry) => entry.id)).toEqual(['open', 'apart-1', 'edit', 'rename', 'apart-4', 'delete']);
    expect(entries.filter((entry) => 'type' in entry && entry.type === 'separator')).toHaveLength(2);
    expect(entriesOf([null], [false])).toEqual([]);
    // The first group has no line before it, and the last none after it.
    expect(entries[0]).toMatchObject({ id: 'open' });
    expect(entries.at(-1)).toMatchObject({ id: 'delete', danger: true });
  });

  it('say in the entry what the act says: why it cannot be done now, what it does, the key that does the same', () => {
    const run = vi.fn();
    const entry = entryOf(act('delete', { danger: true, disabled: 'Units were made from it.', description: 'Removes it.', shortcut: 'Delete', run }));
    expect(entry).toMatchObject({ id: 'delete', danger: true, disabled: 'Units were made from it.', description: 'Removes it.', shortcut: 'Delete' });
    // Choosing the entry does the act.
    if (!('onSelect' in entry)) throw new Error('an act is an entry that can be chosen');
    entry.onSelect();
    expect(run).toHaveBeenCalledTimes(1);
    // What the act does not say is not in the entry.
    expect(Object.keys(entryOf(act('open')))).toEqual(['id', 'label', 'onSelect']);
  });

  it('answer to a key only where the act has one and can be done now', () => {
    const rename = act('rename', { shortcut: 'F2' });
    expect(Object.keys(keysOf(rename, act('open'), null, false))).toEqual(['F2']);
    expect(keysOf(rename).F2).toBe(rename.run);
    expect(keysOf(act('rename', { shortcut: 'F2', disabled: 'Not now.' }))).toEqual({});
  });
});

describe('the list that the way back returns to', () => {
  it('is the list as it was left: what was typed and what was chosen, and nothing that is empty', () => {
    const kind: ResourceKind = 'machines';
    expect(listAsLeft(kind)).toBeUndefined();
    rememberList(kind, { q: 'build', status: undefined, state: '' });
    expect(listAsLeft(kind)).toEqual({ q: 'build' });
    rememberList(kind, { q: 'build', state: 'online' });
    expect(listAsLeft(kind)).toEqual({ q: 'build', state: 'online' });
  });

  it('is kept for each kind apart, and a list that was cleared is the plain list again', () => {
    rememberList('providers', { status: 'ready' });
    rememberList('hosts', { state: 'closed' });
    expect(listAsLeft('providers')).toEqual({ status: 'ready' });
    expect(listAsLeft('hosts')).toEqual({ state: 'closed' });
    rememberList('providers', { status: undefined });
    expect(listAsLeft('providers')).toBeUndefined();
    expect(listAsLeft('hosts')).toEqual({ state: 'closed' });
  });
});

interface Catalogue {
  [key: string]: string | Catalogue;
}

/** What the catalogue says under a dotted key, or nothing. */
function said(catalogue: Catalogue, key: string): string | undefined {
  let at: string | Catalogue | undefined = catalogue;
  for (const part of key.split('.')) {
    if (at === undefined || typeof at === 'string') return undefined;
    at = at[part];
  }
  return typeof at === 'string' ? at : undefined;
}

/** The same nested catalogue expression may point to different words in each language. No fallback is allowed. */
async function hintWordFaults(en: Catalogue, zh: Catalogue, keys: readonly string[]): Promise<string[]> {
  const instance = createInstance();
  await instance.init({ lng: 'en', fallbackLng: false, resources: { en: { translation: en }, 'zh-CN': { translation: zh } },
    saveMissing: true, missingKeyHandler: (_languages, _namespace, key) => { throw new Error(`Missing hint translation: ${key}`); } });
  const faults: string[] = [];
  for (const key of keys) {
    const raw = [said(en, key), said(zh, key)];
    if (raw.some((word) => word === undefined || word.trim() === '')) { faults.push(key); continue; }
    const translated = (lng: string): string => instance.t(key, { lng, defaultValue: '' });
    const englishWord = translated('en');
    const chineseWord = translated('zh-CN');
    if (englishWord.trim() === '' || chineseWord.trim() === '' || englishWord === chineseWord) faults.push(key);
  }
  return faults;
}

describe('the hints', () => {
  const ids = [...new Set(Object.values(HINTED_FORMS).flat())];

  it('are in no form twice over, and every form that has one is listed', () => {
    for (const [form, list] of Object.entries(HINTED_FORMS)) {
      expect(new Set(list).size, form).toBe(list.length);
      expect(list.length, form).toBeGreaterThan(0);
    }
  });

  it('have a term and a sentence in both languages, and the Chinese is not the English', async () => {
    const keys = ids.flatMap((id) => ['title', 'what'].map((part) => `help.hints.${id}.${part}`));
    const lacking = await hintWordFaults(english, chinese, keys);
    expect(lacking, lacking.join('\n')).toEqual([]);
  });

  it('follows translated hint aliases and still refuses copied English or a missing alias target', async () => {
    expect(await hintWordFaults({ title: '$t(term)', term: 'Tools' }, { title: '$t(term)', term: '工具' }, ['title'])).toEqual([]);
    expect(await hintWordFaults({ title: '$t(term)', term: 'Tools' }, { title: '$t(term)', term: 'Tools' }, ['title'])).toEqual(['title']);
    await expect(hintWordFaults({ title: '$t(missing)' }, { title: '工具' }, ['title'])).rejects.toThrow('Missing hint translation');
  });

  it('are given for the settings that have words of their own, in both languages', async () => {
    const instance = createInstance();
    await instance.init({ lng: 'en', fallbackLng: false, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
    const englishEntries = settingHelpEntries(instance);
    await instance.changeLanguage('zh-CN');
    const chineseEntries = settingHelpEntries(instance);
    // In-place hints use the registry's localized definition, not the retired help.settings copy.
    const lacking = SETTING_HINTS.filter((key) => {
      const en = settingDefinitionForKey(englishEntries, key), zh = settingDefinitionForKey(chineseEntries, key);
      if (en === undefined || zh === undefined || en.key !== key || zh.key !== key) return true;
      return en.title.trim() === '' || zh.title.trim() === '' || en.title === key || zh.title === key
        || en.description.trim() === '' || zh.description.trim() === '' || en.description === zh.description;
    });
    expect(lacking).toEqual([]);
    expect(hasSettingHint('runtime.idle_stop_minutes')).toBe(true);
    expect(settingDefinitionForKey(englishEntries, 'runtime.idle_stop_minutes')?.key).toBe('runtime.idle_stop_minutes');
    expect(settingDefinitionForKey(chineseEntries, 'runtime.idle_stop_minutes')?.key).toBe('runtime.idle_stop_minutes');
    // A setting that has no words has no "?": nothing is invented for it.
    expect(hasSettingHint('some.unknown.key')).toBe(false);
    expect(settingDefinitionForKey(englishEntries, 'some.unknown.key')).toBeUndefined();
    expect(settingDefinitionForKey(chineseEntries, 'some.unknown.key')).toBeUndefined();
  });
});

describe('a machine that is to be enrolled again', () => {
  const NOW = Date.parse('2026-10-12T12:00:00Z');

  it('is one that was revoked, or whose certificate has ended', () => {
    expect(lostEnrollment({ status: 'revoked', cert_not_after: '2027-01-01T00:00:00Z' }, NOW)).toBe('revoked');
    expect(lostEnrollment({ status: 'active', cert_not_after: '2026-10-01T00:00:00Z' }, NOW)).toBe('lapsed');
    expect(lostEnrollment({ status: 'active', cert_not_after: '2026-10-12T12:00:00Z' }, NOW)).toBe('lapsed');
    expect(lostEnrollment({ status: 'active', cert_not_after: '2026-10-12T12:00:01Z' }, NOW)).toBeNull();
  });

  it('keeps what is registered from it, which is counted for the dialog to say', () => {
    expect(registeredCount({ agents: [], providers: [], environments: [] })).toBe(0);
    const one = (id: string) => ({ id, name: id });
    expect(registeredCount({ agents: [one('a')], providers: [one('p'), one('q')], environments: [one('e')] } as unknown as Pick<Machine, 'agents' | 'providers' | 'environments'>)).toBe(4);
  });
});

describe('a directory in a small place', () => {
  it('keeps the directory names while concealing the absolute account prefix', () => {
    expect(shortRoot('/home/ada/work/main')).toBe('…/work/main');
    expect(shortRoot('/srv/builds/projects/orchestra/dev/world2/work/env1')).toBe('…/work/env1');
    // A long name with no parts to leave out is kept whole: nothing is cut in the middle of a word.
    expect(shortRoot('/a-directory-with-one-very-long-name-and-nothing-else')).toBe('a-directory-with-one-very-long-name-and-nothing-else');
  });
});
