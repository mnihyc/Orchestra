/** Runtime secrets remain write-only on every runtime; unit controls follow reported lifecycle facts. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { setProviderSecret } from '@/api/resources';
import type * as ResourceApi from '@/api/resources';
import type { SettingsTarget } from '@/api/settings';
import type { ResolvedSetting } from '@/generated/api-types';
import { SETTINGS_REGISTRY } from '@/generated/settings-registry';
import { localizedSetting } from '@/features/settings/settingText';
import { i18n } from '@/i18n';
import { RuntimeConfiguration } from '@/features/resources/providers/RuntimeConfiguration';
import { planProvider } from '../unit/provider-plan-fixtures';
import { show } from './support';

const server = vi.hoisted(() => ({ read: vi.fn<() => Promise<ResolvedSetting[]>>() }));
vi.mock('@/api/resources', async (original) => {
  const actual = await original<typeof ResourceApi>();
  return { ...actual, setProviderSecret: vi.fn() };
});
vi.mock('@/api/settings', async (original) => ({ ...await original<object>(),
  registryQuery: { queryKey: ['settings-registry'], queryFn: () => Promise.resolve([...SETTINGS_REGISTRY]) },
  targetSettingsQuery: (target: SettingsTarget) => ({ queryKey: ['settings', target.kind, 'id' in target ? target.id : 'system'], queryFn: server.read }),
}));
vi.mock('@/features/settings/Editor', () => ({ SettingsEditor: () => null }));
vi.mock('@/features/help/SettingHint', () => ({ SettingHelpLink: () => null }));
vi.mock('@/features/help/ProviderGuideLink', () => ({ ProviderGuideLink: () => null }));

beforeEach(() => {
  vi.clearAllMocks();
  server.read.mockResolvedValue([{ key: 'runtime.secret_names', value: ['API_TOKEN'], own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [] }]);
  vi.mocked(setProviderSecret).mockResolvedValue(undefined);
});

it('writes a named secret separately, conceals its value and clears the field after confirmation', async () => {
  const user = userEvent.setup();
  const provider = planProvider({ secrets_set: ['runtime:API_TOKEN'] });
  show(<RuntimeConfiguration provider={provider} />);
  const value = await screen.findByLabelText('API_TOKEN');
  expect(value).toHaveAttribute('type', 'password');
  expect(value).toHaveValue('');
  await user.type(value, 'only-written-once');
  await user.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(setProviderSecret).toHaveBeenCalledWith(provider.id, 'runtime:API_TOKEN', 'only-written-once'));
  await waitFor(() => expect(value).toHaveValue(''));
  expect(screen.queryByText('only-written-once')).not.toBeInTheDocument();
});

it('a reader sees the secret name without a value field or save button', async () => {
  server.read.mockResolvedValue([{ key: 'runtime.secret_names', value: ['API_TOKEN'], own: null, source: { scope: 'default', node: null }, locked_by: null, writable: false, parts: [] }]);
  const provider = planProvider({ access: { permissions: ['provider.view'], administers: false, through: [],admin_only:[],participating:true } });
  show(<RuntimeConfiguration provider={provider} />);
  const rows = await screen.findByRole('list', { name: 'Unit configuration' });
  expect(rows).toHaveTextContent('API_TOKEN');
  expect(screen.queryByLabelText('API_TOKEN')).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Save' })).not.toBeInTheDocument();
  expect(setProviderSecret).not.toHaveBeenCalled();
});

it.each(['managed','connected'] as const)('a directory provider reads the same runtime settings with %s display placement', async (placement) => {
  show(<RuntimeConfiguration provider={planProvider({placement,runtime_support:{lifecycle:'attach',commands_after_start:{supported:true,reason:null},after_up:false}})} />);
  expect(screen.getByText(/Runtime variables and sealed secrets override the agent profile/)).toBeVisible();
  const definition = SETTINGS_REGISTRY.find((row) => row.key === 'runtime.secret_names');
  if (definition === undefined) throw new Error('The fixture registry has no runtime secret names');
  const rows = await screen.findByRole('list',{name:'Unit configuration'});
  expect(rows).toHaveTextContent(localizedSetting(definition, i18n).title);
  expect(rows).not.toHaveTextContent('runtime.secret_names');
  expect(await screen.findByLabelText('API_TOKEN')).toHaveAttribute('type','password');
  expect(server.read).toHaveBeenCalled();
});

it('a failed provider start shows its repair link and keeps command output behind its disclosure', async () => {
  const user = userEvent.setup();
  show(<RuntimeConfiguration provider={planProvider({ status_detail: 'its runtime could not be made (after_up): after-start command 10-tools exited with 9: tools are unavailable' })} />);
  expect(screen.getByRole('link', { name: 'Change commands after start' })).toHaveAttribute('href', '#unit-after-up');
  expect(screen.queryByText(/tools are unavailable/)).not.toBeInTheDocument();
  const output = screen.getByText(i18n.t('unitConfiguration.commandOutput'), { exact: true });
  expect(output.closest('details')).not.toHaveAttribute('open');
  await user.click(output);
  expect(output.closest('details')).toHaveAttribute('open');
  expect(await screen.findByText(/after-start command 10-tools exited with 9: tools are unavailable/)).toBeVisible();
});
