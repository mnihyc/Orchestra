/** Resource values use the same guarded row as place settings, rather than a second read-only table. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { putSetting } from '@/api/settings';
import { SettingsTable } from '@/features/resources/SettingsTable';
import type { ResolvedSetting, SettingDef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const server = vi.hoisted(() => ({ registry: vi.fn<() => Promise<SettingDef[]>>() }));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), putSetting: vi.fn(),
  registryQuery: { queryKey: ['settings-registry'], queryFn: server.registry },
}));
const def: SettingDef = { key: 'test.keepalive', title: 'Keep alive', description: 'Keep this runtime available.',
  group: 'environment', class: 'parameter', scopes: ['environment'], merge: 'replace', default: false,
  example: true, schema: { type: 'boolean' }, text_key: null, keywords: [] };
const row: ResolvedSetting = { key: def.key, value: false, own: { value: false, revision: 2, locked: false },
  source: { scope: 'environment_own', node: null }, locked_by: null, writable: true, parts: [] };
const target = { kind: 'environment', id: 'workspace' } as const;
const place = { name: 'Workspace', kind: null, owner: null, access: { permissions: ['environment.manage'] } };
beforeEach(async () => {
  await setLocale('en');
  vi.clearAllMocks();
  server.registry.mockResolvedValue([def]);
  vi.mocked(putSetting).mockResolvedValue({ ...row, value: true, own: { value: true, revision: 3, locked: false } });
});
afterEach(async () => { await setLocale('en'); });

it.each(['en', 'zh-CN'] as const)('edits the served writable value once in its row in %s', async (locale) => {
  await setLocale(locale);
  const user = userEvent.setup();
  show(<SettingsTable settings={[row]} label="Runtime" target={target} place={place} />);
  const field = await screen.findByRole('switch', { name: def.title });
  expect(field).not.toBeChecked();
  expect(screen.queryByRole('table')).not.toBeInTheDocument();
  expect(screen.getAllByText(def.description)).toHaveLength(1);
  expect(screen.getAllByRole('switch')).toHaveLength(1);
  await user.click(field);
  await waitFor(() => expect(putSetting).toHaveBeenCalledExactlyOnceWith(target, def.key, { value: true, expectedRevision: 2 }));
  expect(screen.getByRole('switch', { name: def.title })).toBeChecked();
});

it('keeps a declared apply constraint once in the same row', async () => {
  show(<SettingsTable settings={[row]} label="Runtime" target={target} place={place} effectFor={() => 'Next start'} />);
  await screen.findByRole('switch', { name: def.title });
  expect(screen.getAllByText('Next start')).toHaveLength(1);
});

it('makes a missing definition recoverable without presenting a raw key as a name or pretending access was refused', async () => {
  server.registry.mockResolvedValueOnce([]).mockResolvedValueOnce([def]);
  const user = userEvent.setup();
  show(<SettingsTable settings={[row]} label="Runtime" target={target} place={place} />);
  expect(await screen.findByText(i18n.t('settings.compact.definitionMissing'))).toBeVisible();
  expect(screen.queryByText(def.key)).not.toBeInTheDocument();
  expect(screen.queryByText(i18n.t('settings.compact.readOnly'))).not.toBeInTheDocument();
  expect(screen.queryByRole('switch')).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
  expect(await screen.findByRole('switch', { name: def.title })).toBeEnabled();
  expect(putSetting).not.toHaveBeenCalled();
});
