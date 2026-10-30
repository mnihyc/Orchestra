/** A first online connection completes enrollment through the existing live cache, once. */
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
import type * as ToastApi from '@/ui/Toast';
import { TooltipProvider } from '@/ui/Tooltip';
import { enrollmentMachine as machine, enrollmentHost as host } from '../doubles/enrollment-row';

const live = vi.hoisted(() => ({
  navigate: vi.fn<(options: { to: string; params: { id: string } }) => Promise<void>>(() => Promise.resolve()),
  machines: vi.fn<() => Promise<Machine[]>>(() => Promise.resolve([])),
  hosts: vi.fn<() => Promise<ResourceRows<Host>>>(),
  progress: vi.fn<(receipt: string) => Promise<EnrollmentProgress>>(),
}));
vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => live.navigate,
  Link: ({ children }: { children: ReactNode }) => <a href="#enrolled">{children}</a>,
}));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
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
  const view = render(<StrictMode><QueryClientProvider client={client}><TooltipProvider><Toaster><Enrollment /></Toaster></TooltipProvider></QueryClientProvider></StrictMode>);
  return { client, closed, ...view };
}
async function issueCommand(command = 'INSTALL_CONNECTOR') {
  const button = screen.getByRole('button', { name: i18n.t('machineInstallation.showCommand') });
  await waitFor(() => expect(button).toBeEnabled());
  await userEvent.click(button);
  await screen.findByText(command);
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
  await screen.findByText('INSTALL_CONNECTOR');
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
  expect(screen.queryByText('OLD_COMMAND')).not.toBeInTheDocument();
  expect(live.navigate).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();
  vi.mocked(createMachineToken).mockResolvedValueOnce(token('INSTALL_CONNECTOR', OTHER_RECEIPT));
  await issueCommand();
  // Use the footer's text Close rather than the independent corner icon.
  await userEvent.click(screen.getByText(i18n.t('common.close'), { selector: 'button', exact: true }));
  expect(closed).toHaveBeenCalledTimes(1); expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await receive(() => announceMachine(client, machine('after-close', true), OTHER_RECEIPT));
  expect(live.navigate).not.toHaveBeenCalled(); expect(toast).not.toHaveBeenCalled();
});

it('makes no idle reads and recovers only the current receipt on reconnect', async () => {
  const { closed } = openEnrollment();
  await issueCommand();
  await waitFor(() => expect(live.progress).toHaveBeenCalledExactlyOnceWith(RECEIPT));
  vi.useFakeTimers();
  await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
  expect(live.progress).toHaveBeenCalledTimes(1);
  expect(live.machines).not.toHaveBeenCalled(); expect(live.hosts).not.toHaveBeenCalled();
  vi.useRealTimers();
  const connected = machine('missed-while-disconnected', true);
  live.progress.mockResolvedValueOnce({ receipt: RECEIPT, machine: connected, host: null });
  await receive(() => { tellUserEvent('connected', {}); });
  await waitFor(() => expect(live.navigate).toHaveBeenCalledExactlyOnceWith({ to: '/resources/machines/$id', params: { id: connected.id } }));
  expect(live.progress).toHaveBeenNthCalledWith(2, RECEIPT);
  expect(live.progress).toHaveBeenCalledTimes(2);
  expect(closed).toHaveBeenCalledTimes(1);
  expect(live.machines).not.toHaveBeenCalled(); expect(live.hosts).not.toHaveBeenCalled();
});
