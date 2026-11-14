/** Enroll again withholds unusable commands and notices the retained machine's replacement certificate. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, render, screen, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import userEvent from '@testing-library/user-event';
import type * as GatewayApi from '@/api/gateways';
import type * as ResourcesApi from '@/api/resources';
import { enrollMachineAgain, machineEnrollmentInfoQuery, machineKey } from '@/api/resources';
import { EnrollAgainDialog } from '@/features/resources/machines/EnrollAgain';
import { i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';
import { enrollmentMachine } from '../doubles/enrollment-row';

const info = vi.hoisted((): {secure:boolean;public_url:string;configure:string|undefined;returned:boolean} => ({ secure: false, public_url: 'https://orch.example.com', configure: 'public_url', returned: false }));
vi.mock('@/api/gateways', async (load) => ({ ...await load<typeof GatewayApi>(),
  gatewayCatalogueQuery: { queryKey: ['again-gateway-catalogue'], queryFn: () => Promise.resolve({ revision: 'fixture', gateways: [{ id: 'asia', label: 'Asia tools', region: 'Asia', endpoints: [{ url: 'wss://asia.gateway.example.test', trust: 'pinned' as const }] }] }) },
}));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@/api/resources', async (load) => {
  const actual = await load<typeof ResourcesApi>();
  return { ...actual, enrollMachineAgain: vi.fn(),
    machineEnrollmentInfoQuery: { queryKey: ['again-address-info'], queryFn: () => Promise.resolve({ ...info }) },
    machineQuery: (id: string) => ({ queryKey: actual.machineKey(id), queryFn: () => Promise.resolve({ ...enrollmentMachine(id, false), ...(info.returned ? { online: true, cert_not_after: '2031-01-01T00:00:00Z' } : {}) }) }) };
});
beforeEach(() => {
  vi.clearAllMocks(); info.secure = false; info.configure = 'public_url'; info.returned = false;
  vi.mocked(enrollMachineAgain).mockResolvedValue({ token: 'oenr_again.pin', expires_at: '2099-01-01T00:00:00Z',
    commands: { linux: 'REPLACE_RETAINED_IDENTITY', macos: 'REPLACE_RETAINED_IDENTITY', windows: 'REPLACE_RETAINED_IDENTITY' } });
});
function open(node = false) {
  const machine = enrollmentMachine('retained-machine', false);
  if (node) machine.roles=['node'];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const view = render(<QueryClientProvider client={client}><TooltipProvider><EnrollAgainDialog machine={machine} onClose={vi.fn()} /></TooltipProvider></QueryClientProvider>);
  return { ...view, client, machine };
}
for (const locale of ['en', 'zh-CN'] as const) for (const setting of ['public_url', '[[gateway.endpoints]]']) {
  it(`does not spend a replacement token on an example ${setting} (${locale})`, async () => {
    await setLocale(locale); info.configure = setting;
    const view = open();
    try {
      expect(await screen.findByText(i18n.t('machineInstallation.configureAddress', { setting }))).toBeVisible();
      expect(enrollMachineAgain).not.toHaveBeenCalled();
      expect(screen.queryByRole('button', { name: i18n.t('machines.add.copy') })).not.toBeInTheDocument();
    } finally { view.unmount(); view.client.clear(); await setLocale('en'); }
  });
}
it('issues once when the address becomes configured and sees the same machine come back', async () => {
  const view = open();
  try {
    await screen.findByText('Configure public_url before enrolling a machine');
    info.secure = true; info.configure = undefined;
    await act(async () => { await view.client.invalidateQueries({ queryKey: machineEnrollmentInfoQuery.queryKey }); });
    expect(await screen.findByText('REPLACE_RETAINED_IDENTITY')).toBeVisible();
    expect(enrollMachineAgain).toHaveBeenCalledTimes(1);
    expect(enrollMachineAgain).toHaveBeenCalledWith(view.machine.id, expect.any(String));
    info.returned = true;
    act(() => { view.client.setQueryData(machineKey(view.machine.id), { ...view.machine, online: true, cert_not_after: '2031-01-01T00:00:00Z' }); });
    await waitFor(() => expect(screen.queryByText('REPLACE_RETAINED_IDENTITY')).not.toBeInTheDocument());
    expect(screen.getByText(i18n.t('machines.again.done', { name: view.machine.name }))).toBeVisible();
  } finally { view.unmount(); view.client.clear(); }
});


it('offers the node installer on Linux without connector platform commands', async () => {
  info.secure=true;info.configure=undefined;
  const view=open(true);
  try {
    expect(await screen.findByText('REPLACE_RETAINED_IDENTITY')).toBeVisible();
    expect(screen.queryByRole('tab')).not.toBeInTheDocument();
    expect(screen.getAllByRole('button',{name:i18n.t('machines.add.copy')})).toHaveLength(1);
  } finally {view.unmount();view.client.clear();}
});

for (const locale of ['en', 'zh-CN'] as const) for (const node of [false, true]) {
  it(`keeps or replaces the ${node ? 'node' : 'connector'} gateway without replacing its enrollment token (${locale})`, async () => {
    await setLocale(locale); info.secure = true; info.configure = undefined;
    const view = open(node);
    const user = userEvent.setup();
    try {
      expect(await screen.findByText('REPLACE_RETAINED_IDENTITY')).toBeVisible();
      const choice = screen.getByRole('combobox', { name: i18n.t('regionalGateways.gateway') });
      await waitFor(() => expect(choice).toHaveValue(i18n.t('regionalGateways.keepCurrent')));
      await waitFor(() => expect(choice).toBeEnabled());
      await user.click(choice);
      await user.click(screen.getByRole('option', { name: i18n.t('regionalGateways.automatic') }));
      expect(screen.getByText('REPLACE_RETAINED_IDENTITY --gateway automatic')).toBeVisible();
      await user.click(choice);
      await user.click(screen.getByRole('option', { name: 'Asia tools' }));
      expect(screen.getByText('REPLACE_RETAINED_IDENTITY --gateway asia')).toBeVisible();
      if (!node) {
        await user.click(screen.getByRole('tab', { name: 'Windows' }));
        expect(screen.getByText('REPLACE_RETAINED_IDENTITY -Gateway asia')).toBeVisible();
        await user.click(screen.getByRole('tab', { name: 'macOS' }));
        expect(screen.getByText('REPLACE_RETAINED_IDENTITY --gateway asia')).toBeVisible();
      }
      await user.click(choice);
      await user.click(screen.getByRole('option', { name: i18n.t('regionalGateways.keepCurrent') }));
      expect(screen.getByText('REPLACE_RETAINED_IDENTITY')).toBeVisible();
      expect(enrollMachineAgain).toHaveBeenCalledTimes(1);
      expect(enrollMachineAgain).toHaveBeenCalledWith(view.machine.id, expect.any(String));
    } finally { view.unmount(); view.client.clear(); await setLocale('en'); }
  });
}
