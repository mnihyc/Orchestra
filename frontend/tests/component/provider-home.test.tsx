/** The harness-home chooser uses machine facts on either runtime lifecycle and preserves write authority. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { putSetting } from '@/api/settings';
import { ProviderHome } from '@/features/resources/providers/Home';
import type { MachineEnvironmentData, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const { read, write } = vi.hoisted(() => ({ read: vi.fn<() => Promise<MachineEnvironmentData>>(), write: vi.fn() }));
vi.mock('@/api/resource-tools', () => ({ machineEnvironmentDataQuery: (id: string) => ({ queryKey: ['machine-environment-data', id], queryFn: read }) }));
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(), putSetting: write }));
vi.mock('@/features/resources/SettingsTable', () => ({ SettingsTable: () => null }));
const setting = (): ResolvedSetting => ({ key: 'runtime.home', value: { kind: 'made_by_orch' }, own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] });
beforeEach(() => {
  vi.clearAllMocks();
  read.mockResolvedValue({ machine_id: 'physical', roots: [{ path: '/srv/harness', uses: [] }], direct: { made_by_orch: 'directory', existing_directory: true, reason: null }, drivers: { sandbox: { made_by_orch: 'volume', existing_directory: true, reason: null } } });
  vi.mocked(putSetting).mockResolvedValue(setting());
});
afterEach(async () => { await setLocale('en'); });

for (const lifecycle of ['unit', 'attach'] as const) it(`offers the same home choice using ${lifecycle} declarations`, async () => {
  const user = userEvent.setup();
  const storage = { made_by_orch: lifecycle === 'unit' ? 'volume' as const : 'directory' as const, existing_directory: true, reason: null };
  if (lifecycle === 'attach') read.mockResolvedValue({ machine_id: 'physical', roots: [{ path: '/srv/harness', uses: [] }], direct: null, drivers: {} });
  const provider = planProvider({ runtime_support: { data_storage: storage, machine_id: 'physical', driver: lifecycle === 'unit' ? 'sandbox' : null, home: { kind: 'made_by_orch' }, lifecycle, commands_after_start: null, after_up: true } });
  show(<ProviderHome provider={provider} setting={setting()} />);
  await waitFor(() => expect(read).toHaveBeenCalled());
  expect(screen.queryByRole('combobox', { name: 'Workspace' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('combobox', { name: 'Home' }));
  const existing = screen.getByRole('option', { name: i18n.t('choiceCatalog.dataSource.existing_directory.name') });
  expect(existing).toHaveAccessibleDescription(i18n.t('environmentData.allowed'));
  await user.click(existing);
  const input = screen.getByRole('textbox', { name: 'Directory on this machine' });
  await user.clear(input); await user.type(input, '/srv/harness/team');
  await user.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(putSetting).toHaveBeenCalledWith({ kind: 'provider', id: provider.id }, 'runtime.home', { value: { kind: 'existing_directory', path: '/srv/harness/team' }, expectedRevision: null }));
  expect(screen.queryByText(/Private connector state stays separate/)).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: i18n.t('help.hint.label', { term: i18n.t('environmentData.home') }) }));
  expect(await screen.findByText(/Private connector state stays separate/)).toBeVisible();
});

for (const language of ['en', 'zh-CN'] as const) it(`shows the borrowed home without a writable control for a reader (${language})`, async () => {
  await setLocale(language);
  const current = setting(); current.writable = false; current.value = { kind: 'existing_directory', path: '/srv/harness/team' };
  show(<ProviderHome provider={planProvider({ runtime_support: { machine_id: 'physical', driver: null, lifecycle: 'attach', commands_after_start: null, after_up: false } })} setting={current} />);
  expect(await screen.findByDisplayValue('/srv/harness/team')).toBeDisabled();
  expect(screen.queryByRole('button', { name: language === 'en' ? 'Save' : '保存' })).not.toBeInTheDocument();
  expect(putSetting).not.toHaveBeenCalled();
});
