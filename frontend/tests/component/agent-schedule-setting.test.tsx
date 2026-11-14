/** Native setting rows keep off/inheritance/locks and guarded writes, with compact translated help. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { putSetting } from '@/api/settings';
import type * as Settings from '@/api/settings';
import { SettingsTable } from '@/features/resources/SettingsTable';
import { localizedSetting } from '@/features/settings/settingText';
import { authoredSettingGuide } from '@/features/help/settings';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';
const calls = vi.hoisted(() => ({ registry: vi.fn<() => Promise<SettingDef[]>>() }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(), putSetting: vi.fn<typeof Settings.putSetting>(), registryQuery: { queryKey: ['settings-registry'], queryFn: calls.registry } }));
const def: SettingDef = { key: 'threads.agent_schedules', title: 'Agent schedules', description: 'Fallback help', group: 'agent', class: 'parameter', scopes: ['system', 'org', 'project', 'topic', 'thread'], merge: 'replace', default: 'off',
  example: 'on', schema: { type: 'string', enum: ['on', 'off'] }, text_key: null, keywords: [] };
const resolved: ResolvedSetting = { key: def.key, value: 'off', own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] };
const target = { kind: 'node', id: 'schedule-thread' } as const;
const place = { name: 'Review', kind: 'thread', owner: null, access: { permissions: ['thread.manage'] } };
beforeEach(() => { vi.clearAllMocks(); calls.registry.mockResolvedValue([def]); vi.mocked(putSetting).mockResolvedValue({ ...resolved, value: 'on', own: { value: 'on', revision: 1, locked: false }, source: { scope: 'thread', node: { id: target.id, kind: 'thread', name: place.name, redacted: false } } }); });
afterEach(async () => { cleanup(); await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('shows native Off and changes only the guarded setting in %s, with help on request', async (locale) => {
  await setLocale(locale); const title = i18n.t('settingRegistry.threads.agent_schedules.title'), help = i18n.t('settingRegistry.threads.agent_schedules.description');
  const user = userEvent.setup(); show(<SettingsTable settings={[resolved]} target={target} place={place} label="Settings" />);
  const field = await screen.findByRole('combobox', { name: title });
  expect(screen.queryByText(help)).toBeNull(); expect(putSetting).not.toHaveBeenCalled();
  expect(field).toHaveValue(i18n.t('settings.editor.words.off'));
  await user.click(field); await user.click(screen.getByRole('option', { name: i18n.t('settings.editor.words.on') }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: 'on', expectedRevision: null }));
});

it.each(['en', 'zh-CN'] as const)('localizes all three native controls and keeps duration/run defaults replaceable in %s', async (locale) => {
  await setLocale(locale);
  for (const [key, value] of [['threads.agent_schedules', 'off'], ['threads.agent_schedule_hours', 24], ['threads.agent_schedule_runs', 20]] as const) {
    const native: SettingDef = { ...def, key, default: value, schema: typeof value === 'number' ? { type: 'integer', minimum: 1 } : def.schema };
    const localized = localizedSetting(native, i18n);
    expect(localized.title).toBe(i18n.t(`settingRegistry.${key}.title`));
    expect(localized.description).toBe(i18n.t(`settingRegistry.${key}.description`));
    expect(localized.default).toBe(value); expect(localized.schema).not.toHaveProperty('maximum');
    expect(authoredSettingGuide(key)).toEqual({ article: 'agent-schedules', heading: 'enable-agent-schedules', title: 'agentSchedules.title' });
  }
});

it('renders an inherited locked-off value for a viewer without an editable choice', async () => {
  const row: ResolvedSetting = { ...resolved, writable: false, source: { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'Release', redacted: false } }, locked_by: { scope: 'topic', node: { id: 'topic', kind: 'topic', name: 'Release', redacted: false } } };
  show(<SettingsTable settings={[row]} target={target} place={place} label="Settings" />);
  await screen.findByText(i18n.t('settingRegistry.threads.agent_schedules.title'));
  expect(screen.queryByRole('combobox')).toBeNull(); expect(putSetting).not.toHaveBeenCalled();
});
