/** Personal-machine management reviews an audited named grant before any administrative act. */
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Access from '@/api/access';
import type * as Auth from '@/api/auth';
import type * as Resources from '@/api/resources';
import { useMachineManagement } from '@/features/resources/machines/Management';
import { MachineRoles } from '@/features/resources/machines/Roles';
import type { Machine, MachineRolesChange, Me, PageChange, PagePreview, PermissionPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { Button } from '@/ui/Button';
import { enrollmentMachine } from '../doubles/enrollment-row';
import { me, userRef } from '../doubles/fixtures';
import { show } from './support';

const mocks = vi.hoisted(() => ({
  me: vi.fn<() => Promise<Me>>(),
  page: vi.fn<() => Promise<PermissionPage>>(),
  preview: vi.fn<(_target: { id: string }, _change: PageChange) => Promise<PagePreview>>(),
  change: vi.fn<(_target: { id: string }, _change: PageChange) => Promise<PermissionPage>>(),
  act: vi.fn(),
  roles: vi.fn<(_id: string, _roles: readonly string[]) => Promise<MachineRolesChange>>(),
}));
vi.mock('@/features/help/open', () => ({ useHelpOpener: () => vi.fn() }));
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: ReactNode }) => <a href="/resources/machines/personal?tab=policy">{children}</a> }));
vi.mock('@/api/resources', async (load) => ({ ...await load<typeof Resources>(), setMachineRoles: mocks.roles }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['management-caller'], queryFn: () => mocks.me() } }));
vi.mock('@/api/access', async (load) => {
  const actual = await load<typeof Access>();
  return { ...actual, previewPage: mocks.preview, changePage: mocks.change,
    pageQuery: (target: { kind: 'machine'; id: string }) => ({ queryKey: actual.pageKey(target), queryFn: () => mocks.page() }) };
});
vi.mock('@/ui/PersonCard', () => ({ PersonCard: ({ children }: { children: ReactNode }) => children }));

const administrator = me(99, 'admin', 'Current administrator', { is_system_admin: true });
function page(): PermissionPage {
  return { object: { kind: 'machine', id: 'personal', name: 'build-vm' }, page: 'live', owner: null, owners: [], owners_writable: false,
    external: true, admins: { count: 1, of: ['system'] }, rows: [], state: 'follows', followers: null,
    gives: { choices: [], roles: [], agents: [], holders_share: true }, covers: null, depends: [], reaches: null, revision: 7 };
}
function machine(): Machine {
  return { ...enrollmentMachine('personal', false), access: { administers: true, permissions: ['machine.view', 'access.manage'], through: [], admin_only: ['machine.view', 'access.manage'], admin_place: 'system' } };
}
function Entrance({ value = machine() }: { value?: Machine }) {
  const management = useMachineManagement();
  return <><Button onClick={() => management.run(value, mocks.act)}>Enroll again</Button>{management.dialog}</>;
}
beforeEach(() => {
  vi.clearAllMocks();
  mocks.me.mockResolvedValue(administrator); mocks.page.mockResolvedValue(page());
  mocks.preview.mockResolvedValue({ pages: 1, orgs: null, gaining: [{ user: userRef(administrator), permissions: ['machine.manage'], pages: 1 }], losing: [], more: 0 });
  mocks.change.mockResolvedValue(page());
});
afterEach(async () => { delete document.documentElement.dataset.theme; await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark']) it(`names the caller and writes only the confirmed Machine Manage grant (${locale}, ${theme})`, async () => {
  await setLocale(locale); document.documentElement.dataset.theme = theme;
  show(<Entrance />); const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: 'Enroll again' }));
  const confirm = await screen.findByRole('button', { name: i18n.t('machineManagement.continue') });
  await waitFor(() => expect(confirm).toBeEnabled());
  expect(screen.getByText('Current administrator')).toBeVisible();
  expect(screen.getByText(i18n.t('machineManagement.audit'))).toBeVisible();
  expect(mocks.change).not.toHaveBeenCalled(); expect(mocks.act).not.toHaveBeenCalled();
  await user.click(confirm); await waitFor(() => expect(mocks.act).toHaveBeenCalledTimes(1));
  expect(mocks.change).toHaveBeenCalledExactlyOnceWith({ kind: 'machine', id: 'personal' }, { expected_revision: 7, rows: [{ permission: 'machine.manage', add: [{ holder: { kind: 'user', id: administrator.id } }] }] });
});

it('cancels without a grant or machine act', async () => {
  show(<Entrance />); const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: 'Enroll again' }));
  await screen.findByRole('button', { name: 'Add and continue' }); await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(mocks.change).not.toHaveBeenCalled(); expect(mocks.act).not.toHaveBeenCalled();
});

it('executes no machine act after a refused grant', async () => {
  mocks.change.mockRejectedValue(new Error('The session no longer administers this machine'));
  show(<Entrance />); const user = userEvent.setup(); await user.click(screen.getByRole('button', { name: 'Enroll again' }));
  const confirm = await screen.findByRole('button', { name: 'Add and continue' }); await waitFor(() => expect(confirm).toBeEnabled());
  await user.click(confirm); await waitFor(() => expect(mocks.change).toHaveBeenCalledTimes(1));
  expect(mocks.act).not.toHaveBeenCalled(); expect(screen.getByRole('alertdialog')).toBeVisible();
});

it('blocks confirmation when its preview failed', async () => {
  mocks.preview.mockRejectedValue(new Error('The machine page changed'));
  show(<Entrance />); await userEvent.click(screen.getByRole('button', { name: 'Enroll again' }));
  const confirm = await screen.findByRole('button', { name: 'Add and continue' }); await waitFor(() => expect(mocks.preview).toHaveBeenCalledTimes(1));
  expect(confirm).toBeDisabled(); expect(mocks.change).not.toHaveBeenCalled(); expect(mocks.act).not.toHaveBeenCalled();
});

for (const node of [false, true]) it(`needs no override for an already held ${node ? 'Host' : 'Machine'} Manage permission`, async () => {
  const value = enrollmentMachine('held', false);
  if (node) { value.roles = ['node']; value.access = { administers: true, permissions: ['host.manage'], through: [] }; }
  show(<Entrance value={value} />); await userEvent.click(screen.getByRole('button', { name: 'Enroll again' }));
  expect(mocks.act).toHaveBeenCalledTimes(1); expect(mocks.preview).not.toHaveBeenCalled(); expect(mocks.change).not.toHaveBeenCalled();
});

it('uses that same reviewed grant before the actual role editor can dispatch a change', async () => {
  const value = { ...machine(), online: true, role_management: { supported: true, delegated: true, enabled: true } };
  mocks.roles.mockResolvedValue({ machine_view: value, requested_roles: ['environment', 'agent', 'provider'], reconnecting: true });
  show(<MachineRoles machine={value} />); const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Change roles' }));
  const confirm = await screen.findByRole('button', { name: 'Add and continue' }); await waitFor(() => expect(confirm).toBeEnabled());
  expect(screen.queryByRole('checkbox', { name: 'Provider' })).not.toBeInTheDocument(); expect(mocks.roles).not.toHaveBeenCalled();
  await user.click(confirm);
  await user.click(await screen.findByRole('checkbox', { name: 'Provider' })); await user.click(screen.getByRole('button', { name: 'Save' }));
  await waitFor(() => expect(mocks.roles).toHaveBeenCalledExactlyOnceWith('personal', ['environment', 'agent', 'provider']));
});
