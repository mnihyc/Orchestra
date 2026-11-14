/** An unchanged machine home remains the native default; a deliberate home choice is explicit. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import type * as Resources from '@/api/resources';
import type { Provider } from '@/generated/api-types';
import { RegisterProviderDialog } from '@/features/resources/machines/RegisterProvider';
import { useTreeStore } from '@/realtime/stores/tree';
import { i18n, setLocale } from '@/i18n';
import { detailAgent, detailHome, detailMachine } from './resource-detail-fixtures';
import { me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const writes = vi.hoisted(() => ({ register: vi.fn<(machine: string, body: Resources.MachineProviderRegistration, key: string) => Promise<Provider>>(() => new Promise<Provider>(() => undefined)) }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), registerProvider: writes.register,
  providerSetupQuery: (home: string) => ({ queryKey: ['machine-setup', home], queryFn: () => Promise.resolve({ protocols: [], http_allowed: false }) }),
}));
vi.mock('@/api/nodes', async (load) => ({ ...await load<typeof Nodes>(),
  fetchTree: () => Promise.resolve({ nodes: [], hidden: [], seq: 1, since: null, complete: true, next_cursor: null }),
  orgsQuery: { queryKey: ['machine-homes'], initialData: () => [org('other-home', 'another', 'Another home', userRef(me(1, 'ada', 'Ada')))], staleTime: Infinity,
    queryFn: () => Promise.resolve([org('other-home', 'another', 'Another home', userRef(me(1, 'ada', 'Ada')))]) },
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<object>(), useNavigate: () => vi.fn(), useLocation: () => ({ pathname: '/machine', searchStr: '' }) }));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
afterEach(async () => { cleanup(); writes.register.mockClear(); useTreeStore.setState({ byOrg: new Map() }); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const override of [false, true]) it(`registers a provider with ${override ? 'the chosen override' : 'the machine default'} in ${locale}`, async () => {
  await setLocale(locale); const user = userEvent.setup();
  const machine = { ...detailMachine, home: detailHome, agents: [detailAgent] };
  show(<RegisterProviderDialog machine={machine} open onClose={vi.fn()} />);
  const home = await screen.findByRole('combobox', { name: i18n.t('resources.home.label') });
  await waitFor(() => expect(home).toHaveDisplayValue(i18n.t('resourceGroups.machineHomeNamed', { name: detailHome.name })));
  expect(writes.register).not.toHaveBeenCalled();
  if (override) {
    await user.click(home); await user.click(screen.getByRole('option', { name: 'Another home' }));
  }
  await user.type(screen.getByRole('textbox', { name: i18n.t('machines.provider.name') }), 'Local account');
  await user.click(screen.getByRole('button', { name: i18n.t('machines.provider.submit') }));
  await waitFor(() => expect(writes.register).toHaveBeenCalledTimes(1));
  expect(writes.register.mock.calls[0]?.[1]).toEqual({ name: 'Local account', kind: 'subscription', agent_id: detailAgent.id,
    ...(override ? { home_id: 'other-home' } : {}) });
});
