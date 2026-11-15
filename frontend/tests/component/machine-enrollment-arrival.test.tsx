/** A first online connection completes enrollment through the existing live cache, once. */
import type * as GatewayApi from '@/api/gateways';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode, useState, type ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { createHostToken, createMachineToken, hostKey, hostsQuery, machineKey, machinesQuery } from '@/api/resources';
import type * as ResourcesApi from '@/api/resources';
import { applyChanges, resourceChanges, userEventChanges } from '@/api/acts';
import type * as EnrollmentApi from '@/api/enrollment';
import { type ResourceRows, resourceRows } from '@/api/listed';
import type { HomePickerProps } from '@/features/resources/HomePicker';
import { AddMachineDialog } from '@/features/resources/machines/Enrollment';
import type { EnrollmentProgress, EnrollmentToken, Host, Machine } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { toast, Toaster } from '@/ui/Toast';
import { tellUserEvent } from '@/realtime/stores/user';
import { READ_AGAIN_DOWN_MS, READ_AGAIN_UP_MS, readAgainFollows } from '@/api/readAgain';
import type * as ToastApi from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { enrollmentMachine as machine, enrollmentHost as host } from '../doubles/enrollment-row';

const live = vi.hoisted(() => ({
  navigate: vi.fn<(options: { to: string; params: { id: string } }) => Promise<void>>(() => Promise.resolve()),
  machines: vi.fn<() => Promise<Machine[]>>(() => Promise.resolve([])),
  hosts: vi.fn<() => Promise<ResourceRows<Host>>>(),
  progress: vi.fn<(receipt: string) => Promise<EnrollmentProgress>>(),
}));
vi.mock('@tanstack/react-router', async (loadOriginal) => ({ ...await loadOriginal<object>(),
  useNavigate: () => live.navigate,
  Link: ({ children }: { children: ReactNode }) => <a href="#enrolled">{children}</a>,
}));
vi.mock('@/api/gateways', async (load) => ({ ...await load<typeof GatewayApi>(),
  gatewayCatalogueQuery: { queryKey: ['enrollment-gateway-catalogue'], queryFn: () => Promise.resolve({ revision: 'fixture', gateways: [] }) },
}));
vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));
vi.mock('@/features/resources/HomePicker', () => ({
  useHomeChoices: () => ({ choices: [{ id: 'org', label: 'Acme' }] }),
  HomePicker: ({ value, onChange }: Pick<HomePickerProps, 'value' | 'onChange'>) => (
    <select aria-label="Home" value={value} onChange={(event) => onChange(event.target.value)}>
      <option value="">Personal</option><option value="org">Acme</option>
    </select>
  ),
}));
vi.mock('@/api/resources', async (load) => {
  const actual = await load<typeof ResourcesApi>();
  return { ...actual,
  createMachineToken: vi.fn(), createHostToken: vi.fn(),
  machineEnrollmentInfoQuery: { ...actual.machineEnrollmentInfoQuery, queryFn: () => Promise.resolve({ secure: true, public_url: 'https://orch.example' }) },
  machinesQuery: { ...actual.machinesQuery, queryFn: live.machines },
  hostsQuery: (org: string | null) => ({ ...actual.hostsQuery(org), queryFn: live.hosts }),
}; });
vi.mock('@/api/enrollment', async (load) => {
  const actual = await load<typeof EnrollmentApi>();
  return { ...actual, enrollmentProgressQuery: (receipt: string) => ({ ...actual.enrollmentProgressQuery(receipt), queryFn: () => live.progress(receipt) }) };
});
// Keep the real visible notification; retain its dismiss function for isolated cases.
vi.mock('@/ui/Toast', async (load) => {
  const actual = await load<typeof ToastApi>();
  return { ...actual, toast: vi.fn(actual.toast) };
});

const RECEIPT = 'a'.repeat(64), OTHER_RECEIPT = 'b'.repeat(64);
function token(command: string, receipt = RECEIPT): EnrollmentToken {
  return { receipt, token: 'oenr_arrival.pin', expires_at: new Date(Date.now() + 600_000).toISOString(), commands: { linux: command, macos: command, windows: command } };
}

const clients: QueryClient[] = [];
const originalTheme = document.documentElement.dataset.theme;
const originalDark = document.documentElement.classList.contains('dark');
beforeEach(() => {
  vi.clearAllMocks();
  live.hosts.mockResolvedValue(resourceRows<Host>([]));
  live.progress.mockImplementation((receipt) => Promise.resolve({ receipt, machine: null, host: null }));
  vi.mocked(createMachineToken).mockReset().mockResolvedValue(token('INSTALL_CONNECTOR'));
  vi.mocked(createHostToken).mockReset().mockResolvedValue(token('INSTALL_NODE'));
});
afterEach(async () => {
  vi.useRealTimers();
  readAgainFollows(() => true);
  cleanup();
  act(() => {
    for (const result of vi.mocked(toast).mock.results) if (result.type === 'return') result.value();
  });
  for (const client of clients.splice(0)) client.clear();
  await setLocale('en');
  if (originalTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = originalTheme;
  document.documentElement.classList.toggle('dark', originalDark);
});

function openEnrollment(kind: 'connector' | 'node' = 'connector', existing: Machine[] = []) {
  // The parent resource list has already loaded. The live feed writes these same query keys.
  const client = new QueryClient({ defaultOptions: { queries: { staleTime: Infinity, retry: false }, mutations: { retry: false } } });
  clients.push(client);
  client.setQueryData(machinesQuery.queryKey, existing);
  client.setQueryData(hostsQuery(null).queryKey, resourceRows<Host>([]));
  const closed = vi.fn();
  function Enrollment() {
    const [open, setOpen] = useState(true);
    return open ? <AddMachineDialog initialKind={kind} onClose={() => { closed(); setOpen(false); }} /> : null;
  }
  const view = render(<StrictMode><QueryClientProvider client={client}><TooltipProvider overflow={false}><Toaster><Enrollment /></Toaster></TooltipProvider></QueryClientProvider></StrictMode>);
  return { client, closed, ...view };
}
async function issueCommand(command = 'INSTALL_CONNECTOR') {
  const button = screen.getByRole('button', { name: i18n.t('machineInstallation.showCommand') });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
  await screen.findByText(`${command} --gateway automatic`);
}
async function receive(update: () => void | Promise<void>) {
  await act(async () => {
    await update();
    // Yield after delivery so React can process the query observer within this act scope.
    await Promise.resolve();
  });
}

function announceMachine(client: QueryClient, row: Machine, receipt = RECEIPT) {
  return applyChanges(userEventChanges('enrollment', { receipt, status: { receipt, machine: row, host: null } }), client);
}
function announceHost(client: QueryClient, row: Host, receipt = RECEIPT) {
  return applyChanges(userEventChanges('enrollment', { receipt, status: { receipt, machine: null, host: row } }), client);
}
function changedMachine(client: QueryClient, row: Machine) {
  return applyChanges(resourceChanges('machine', row.id, { what: 'changed', row: { kind: 'machine', value: row } }), client);
}
function changedHost(client: QueryClient, row: Host) {
  return applyChanges(resourceChanges('host', row.id, { what: 'changed', row: { kind: 'host', value: row } }), client);
}

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`${language}/${theme} waits for online, announces the connection once, and opens the machine without another click or list read`, async () => {
    await setLocale(language);
    document.documentElement.dataset.theme = theme;
    document.documentElement.classList.toggle('dark', theme === 'dark');
    const existing = machine('existing', true);
    const { client, closed } = openEnrollment('connector', [existing]);
    await issueCommand();

    // A saved enrollment is not yet a connected machine.
    const enrolled = machine('new-machine', false);
    await receive(() => announceMachine(client, enrolled));
    expect(screen.getByRole('dialog')).toBeVisible();
    await waitFor(() => expect(screen.getByText(i18n.t('machines.add.arrived', { name: enrolled.name }))).toBeVisible());
    expect(screen.getAllByText(i18n.t('machines.add.waiting'), { exact: true })).toHaveLength(1);
    expect(closed).not.toHaveBeenCalled(); expect(live.navigate).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();

    const connected = { ...enrolled, online: true };
    await receive(() => changedMachine(client, connected));
    await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/machines/$id', params: { id: connected.id } }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByText(i18n.t('machines.add.connected'), { exact: true })).toBeVisible());
    expect(toast).toHaveBeenCalledExactlyOnceWith({ tone: 'success', title: i18n.t('machines.add.connected') });
    expect(closed).toHaveBeenCalledTimes(1);
    expect(client.getQueryData(machineKey(connected.id))).toEqual(connected);

    await receive(() => changedMachine(client, { ...connected }));
    expect(live.navigate).toHaveBeenCalledTimes(1); expect(toast).toHaveBeenCalledTimes(1); expect(closed).toHaveBeenCalledTimes(1);
    expect(live.machines).not.toHaveBeenCalled(); expect(live.hosts).not.toHaveBeenCalled();
  });
}

it.each(['en', 'zh-CN'] as const)('opens the connected Node by host ID with its own announcement (%s)', async (language) => {
  await setLocale(language);
  const { client, closed } = openEnrollment('node');
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Home' }), 'org');
  await issueCommand('INSTALL_NODE');
  const enrolled = host('new-host', false);
  await receive(() => announceHost(client, enrolled));
  await waitFor(() => expect(screen.getByText(i18n.t('machines.add.arrived', { name: enrolled.name }))).toBeVisible());
  expect(screen.getByRole('dialog')).toBeVisible(); expect(live.navigate).not.toHaveBeenCalled();
  const connected = { ...enrolled, machine: { ...enrolled.machine, online: true } };
  await receive(() => changedHost(client, connected));
  await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/hosts/$id', params: { id: connected.id } }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await waitFor(() => expect(screen.getByText(i18n.t('hosts.add.connected'), { exact: true })).toBeVisible());
  expect(toast).toHaveBeenCalledExactlyOnceWith({ tone: 'success', title: i18n.t('hosts.add.connected') });
  expect(closed).toHaveBeenCalledTimes(1); expect(client.getQueryData(hostKey(connected.id))).toEqual(connected);
  expect(createMachineToken).not.toHaveBeenCalled(); expect(live.hosts).not.toHaveBeenCalled();
});

it('ignores earlier arrivals and existing reconnections, including a row arriving while the token is issued', async () => {
  const { client, closed } = openEnrollment();
  const existing = machine('before-command', false);
  await receive(() => { client.setQueryData(machinesQuery.queryKey, [existing]); });
  expect(live.navigate).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();
  let finish: (value: EnrollmentToken) => void = () => { throw new Error('token not requested'); };
  const pending = new Promise<EnrollmentToken>((resolve) => { finish = resolve; });
  vi.mocked(createMachineToken).mockImplementationOnce(() => pending);
  const button = screen.getByRole('button', { name: i18n.t('machineInstallation.showCommand') });
  await waitFor(() => expect(button).toBeEnabled()); await userEvent.click(button);
  const duringRequest = machine('during-request', true);
  await act(async () => {
    client.setQueryData(machinesQuery.queryKey, [{ ...existing, online: true }, duringRequest]);
    finish(token('INSTALL_CONNECTOR')); await pending;
  });
  await screen.findByText('INSTALL_CONNECTOR --gateway automatic');
  expect(screen.getByRole('dialog')).toBeVisible(); expect(closed).not.toHaveBeenCalled(); expect(live.navigate).not.toHaveBeenCalled();
  await receive(() => announceMachine(client, machine('unrelated-after-command', true), OTHER_RECEIPT));
  expect(screen.getByRole('dialog')).toBeVisible(); expect(live.navigate).not.toHaveBeenCalled();
  await receive(() => announceMachine(client, machine('after-command', true)));
  await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/machines/$id', params: { id: 'after-command' } }));
});

it('does not open from a late command for changed consent or from a dialog the person closed', async () => {
  let finish: (value: EnrollmentToken) => void = () => { throw new Error('token not requested'); };
  const pending = new Promise<EnrollmentToken>((resolve) => { finish = resolve; });
  vi.mocked(createMachineToken).mockImplementationOnce(() => pending);
  const { client, closed } = openEnrollment();
  const button = screen.getByRole('button', { name: i18n.t('machineInstallation.showCommand') });
  await waitFor(() => expect(button).toBeEnabled()); await userEvent.click(button);
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Home' }), 'org');
  await act(async () => { finish(token('OLD_COMMAND')); await pending; });
  await receive(() => announceMachine(client, machine('old-consent', true)));
  expect(screen.queryByText('OLD_COMMAND --gateway automatic')).not.toBeInTheDocument();
  expect(live.navigate).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();
  vi.mocked(createMachineToken).mockResolvedValueOnce(token('INSTALL_CONNECTOR', OTHER_RECEIPT));
  await issueCommand();
  // Use the footer's text Close rather than the independent corner icon.
  await userEvent.click(screen.getByText(i18n.t('common.close'), { selector: 'button', exact: true }));
  expect(closed).toHaveBeenCalledTimes(1); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await receive(() => announceMachine(client, machine('after-close', true), OTHER_RECEIPT));
  expect(live.navigate).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();
});

it('reads seldom while the live connection is up, and recovers only the current receipt on reconnect', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const { closed } = openEnrollment();
  await issueCommand();
  await waitFor(() => expect(live.progress).toHaveBeenCalledExactlyOnceWith(RECEIPT));
  await act(async () => { await vi.advanceTimersByTimeAsync(READ_AGAIN_UP_MS - 2_000); });
  expect(live.progress).toHaveBeenCalledTimes(1);
  await act(async () => { await vi.advanceTimersByTimeAsync(3_000); });
  expect(live.progress).toHaveBeenCalledTimes(2);
  expect(live.machines).not.toHaveBeenCalled(); expect(live.hosts).not.toHaveBeenCalled();
  const connected = machine('missed-while-disconnected', true);
  live.progress.mockResolvedValueOnce({ receipt: RECEIPT, machine: connected, host: null });
  await receive(() => { tellUserEvent('connected', {}); });
  await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/machines/$id', params: { id: connected.id } }));
  expect(live.progress).toHaveBeenNthCalledWith(3, RECEIPT);
  expect(live.progress).toHaveBeenCalledTimes(3);
  expect(closed).toHaveBeenCalledTimes(1);
  expect(live.machines).not.toHaveBeenCalled(); expect(live.hosts).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('opens the machine from a later read when no live event ever arrives and the connection is down (%s)', async (language) => {
  await setLocale(language);
  readAgainFollows(() => false);
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const { closed } = openEnrollment();
  await issueCommand();
  await waitFor(() => expect(live.progress).toHaveBeenCalledExactlyOnceWith(RECEIPT));
  // The machine enrols, and nothing tells the dialog: only the next read can.
  const connected = machine('enrolled-unannounced', true);
  live.progress.mockResolvedValue({ receipt: RECEIPT, machine: connected, host: null });
  await act(async () => { await vi.advanceTimersByTimeAsync(READ_AGAIN_DOWN_MS); });
  await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/machines/$id', params: { id: connected.id } }));
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(closed).toHaveBeenCalledTimes(1);
  expect(toast).toHaveBeenCalledExactlyOnceWith({ tone: 'success', title: i18n.t('machines.add.connected') });
});

it('stops reading when the dialog is closed', async () => {
  readAgainFollows(() => false);
  vi.useFakeTimers({ shouldAdvanceTime: true });
  openEnrollment();
  await issueCommand();
  await waitFor(() => expect(live.progress).toHaveBeenCalledExactlyOnceWith(RECEIPT));
  await userEvent.click(screen.getByText(i18n.t('common.close'), { selector: 'button', exact: true }));
  await act(async () => { await vi.advanceTimersByTimeAsync(READ_AGAIN_DOWN_MS * 5); });
  expect(live.progress).toHaveBeenCalledTimes(1);
});

it('opens the host from a later read when no live event ever arrives', async () => {
  readAgainFollows(() => false);
  vi.useFakeTimers({ shouldAdvanceTime: true });
  const { closed } = openEnrollment('node');
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Home' }), 'org');
  await issueCommand('INSTALL_NODE');
  await waitFor(() => expect(live.progress).toHaveBeenCalledExactlyOnceWith(RECEIPT));
  const connected = host('enrolled-unannounced', true);
  live.progress.mockResolvedValue({ receipt: RECEIPT, machine: null, host: { ...connected, machine: { ...connected.machine, online: true } } });
  await act(async () => { await vi.advanceTimersByTimeAsync(READ_AGAIN_DOWN_MS); });
  await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/hosts/$id', params: { id: connected.id } }));
  expect(closed).toHaveBeenCalledTimes(1);
});


for (const kind of ['connector', 'node'] as const) it.each(['en', 'zh-CN'] as const)(`names a new ${kind} before its setup and includes the name in its enrollment token (%s)`, async (locale) => {
  await setLocale(locale);
  openEnrollment(kind);
  const name = screen.getByRole('textbox', { name: `${i18n.t('resources.edit.name')} ${i18n.t('ui.field.optional')}` });
  expect(screen.getByRole('dialog').querySelector('input:not([type="hidden"]),textarea,[role="combobox"],select')).toBe(name);
  const user = userEvent.setup();
  await user.click(name); await user.paste('Compute bench');
  if (kind === 'node') await user.selectOptions(screen.getByRole('combobox', { name: 'Home' }), 'org');
  await issueCommand(kind === 'node' ? 'INSTALL_NODE' : 'INSTALL_CONNECTOR');
  if (kind === 'node') expect(createHostToken).toHaveBeenCalledWith(expect.objectContaining({ name_hint: 'Compute bench' }), expect.any(String));
  else expect(createMachineToken).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ name_hint: 'Compute bench' }));
});
