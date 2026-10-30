/** Palette acts are the current native handlers; their names, destinations and current bindings are searchable. */
import { afterEach, expect, it, vi } from 'vitest';
import { i18n, setLocale } from '@/i18n';
import { menuCommands } from '@/features/finder/menuCommands';
import { commandEntries, commandRows, helpCommands } from '@/features/finder/local';
import { settingsDestinations } from '@/features/finder/settingsCommands';
import { assemble, type Env, type Scored } from '@/features/finder/rows';
import { shortcutMatches } from '@/features/help/shortcutSearch';
import { sideReadingAvailable, sideRefusal } from '@/features/side-threads/availability';
import type { Thread } from '@/generated/api-types';
import type { MenuEntry } from '@/ui/Menu';
import { Pencil } from 'lucide-react';

afterEach(async () => { await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) it(`finds a native switch and honors a newer refusal or handler (${locale})`, async () => {
  await setLocale(locale);
  const t = i18n.getFixedT(locale);
  const old = vi.fn(); const current = vi.fn();
  let entries: MenuEntry[] = [{ id: 'change-switch', label: t('change.menu.switch'), onSelect: old }];
  const commands = menuCommands(entries, () => entries, { prefix: 'thread', name: 'Main' });
  const env: Env = { t, go: vi.fn(), orgSlug: null, exists: () => true, showCard: vi.fn() };
  const found = commandRows(commandEntries(commands, t), locale === 'en' ? 'switch agent' : '切换 智能体', env);
  expect(found).toHaveLength(1);
  expect(found[0]?.row.disabled).toBeUndefined();
  entries = [{ id: 'change-switch', label: t('change.menu.switch'), disabled: 'A job holds this thread', onSelect: old }];
  found[0]?.row.run('here');
  expect(old).not.toHaveBeenCalled();
  expect(commands[0]?.available?.()).toEqual({ disabled: 'A job holds this thread' });
  entries = [{ id: 'change-switch', label: t('change.menu.switch'), onSelect: current }];
  found[0]?.row.run('here');
  expect(current).toHaveBeenCalledOnce();
  expect(old).not.toHaveBeenCalled();
  entries = [];
  found[0]?.row.run('here');
  expect(current).toHaveBeenCalledOnce();
  expect(commands[0]?.available?.()).toBe(false);
});

for (const locale of ['en', 'zh-CN'] as const) it(`omits an unsupported side-thread command and keeps the two distinct fork acts (${locale})`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale);
  const reason = 'The reader has not verified read-only access.';
  // The boundary deliberately includes only fields consumed by the native availability rule.
  const thread = { access: { permissions: ['thread.interact'] }, archived: false, side_of: null, lock: null,
    sides: { allowed: true, mode: null, reason } } as unknown as Thread;
  const ask = vi.fn(); const fork = vi.fn(); const forkTo = vi.fn();
  const entries: MenuEntry[] = [
    ...(sideReadingAvailable(thread) ? [{ id: 'side-ask', label: t('side.menu.ask'), disabled: sideRefusal(thread, t), onSelect: ask }] : []),
    { id: 'change-fork-now', label: t('change.menu.forkNow'), onSelect: fork },
    { id: 'change-fork', label: t('change.menu.fork'), onSelect: forkTo },
  ];
  const commands = menuCommands(entries, () => entries, { prefix: 'thread', name: 'Main' });
  const env: Env = { t, go: vi.fn(), orgSlug: null, exists: () => true, showCard: vi.fn() };
  const aside = commandRows(commandEntries(commands, t), locale === 'en' ? 'ask side' : '侧问', env);
  expect(aside).toHaveLength(0);
  expect(sideRefusal(thread, t)).toBe(t('side.unavailable.readOnly'));
  expect(ask).not.toHaveBeenCalled();
  const branches = commandRows(commandEntries(commands, t), locale === 'en' ? 'fork' : '分叉', env);
  expect(branches).toHaveLength(2);
  branches.find(({ row }) => row.id === 'commands:thread.change-fork-now')?.row.run('here');
  expect(fork).toHaveBeenCalledOnce(); expect(forkTo).not.toHaveBeenCalled();
  branches.find(({ row }) => row.id === 'commands:thread.change-fork')?.row.run('here');
  expect(forkTo).toHaveBeenCalledOnce();
});

for (const locale of ['en', 'zh-CN'] as const) it(`opens each settings section by its real address with a distinct readable name (${locale})`, async () => {
  await setLocale(locale); const t = i18n.getFixedT(locale);
  const org = settingsDestinations({ kind: 'org', id: 'org-id', address: 'acme.research', name: 'Research' }, t);
  expect(new Set(org.map((entry) => entry.title)).size).toBe(org.length);
  expect(org.find((entry) => entry.id === 'members')?.path).toBe('/org/acme.research/settings/members');
  expect(org.some((entry) => entry.id === 'destinations')).toBe(false);
  expect(settingsDestinations({ kind: 'org', id: 'flat', name: 'Flat', flat: true }, t).some((entry) => entry.id === 'departments')).toBe(false);
  expect(org.every((entry) => !entry.title.includes('acme.research') && !entry.title.includes('org-id'))).toBe(true);
  const project = settingsDestinations({ kind: 'project', id: 'project-id', name: 'Website' }, t);
  expect(project.find((entry) => entry.id === 'policies')?.path).toBe('/project/project-id/settings?section=policies');
  const topic = settingsDestinations({ kind: 'topic', id: 'topic-id', name: 'Login' }, t);
  expect(topic.find((entry) => entry.id === 'audit')?.path).toBe('/topic/topic-id?tab=settings&section=audit');
});

it('opens a help-topic act through the single Help handler and never offers a placeholder guide', async () => {
  await setLocale('en'); const open = vi.fn();
  const article = { id: 'getting-started', title: 'Getting started', headings: [], keywords: ['first', 'thread'], screens: ['thread'], body: 'Start here.' };
  const commands = helpCommands([article, { ...article, id: 'not-built', body: 'Placeholder guide.' }], open);
  expect(commands).toHaveLength(1);
  expect(commands[0]?.title(i18n.getFixedT('en'))).toBe('Help: Getting started');
  commands[0]?.run('here'); expect(open).toHaveBeenCalledExactlyOnceWith('getting-started');
});

it('the acts list includes actions after thirty and other local groups really expand', async () => {
  await setLocale('en');
  const rows = Array.from({ length: 45 }, (_, index): Scored => ({ score: 0, row: { id: `act-${index}`, group: 'commands', title: `Action ${index + 1}`, detail: '', marks: [], icon: Pencil, run: vi.fn() } }));
  const before = assemble({ mode: 'commands', empty: true, answers: { commands: rows }, expanded: new Set(), t: i18n.getFixedT('en') });
  expect(before[0]?.rows).toHaveLength(45);
  expect(before[0]?.more).toBe(0);
  const after = assemble({ mode: 'all', empty: true, answers: { commands: rows }, expanded: new Set(['commands']), t: i18n.getFixedT('en') });
  expect(after[0]?.rows).toHaveLength(45);
  expect(after[0]?.more).toBe(0);
  after[0]?.rows[44]?.run('here');
  expect(rows[44]?.row.run).toHaveBeenCalledOnce();
});

it('searches action words, sequences and the physical modifier, without finding a removed binding', () => {
  const row = { name: 'Go to the inbox', where: 'Here', keys: ['Mod+K', 'G I'] };
  expect(shortcutMatches('go inbox', row, false)).toBe(true);
  expect(shortcutMatches('ctrl+k', row, false)).toBe(true);
  expect(shortcutMatches('G I', row, false)).toBe(true);
  expect(shortcutMatches('cmd k', row, true)).toBe(true);
  expect(shortcutMatches('ctrl k', row, true)).toBe(false);
  expect(shortcutMatches('ctrl k', { ...row, keys: [] }, false)).toBe(false);
  expect(shortcutMatches('收件箱', { ...row, name: '前往收件箱' })).toBe(true);
});
