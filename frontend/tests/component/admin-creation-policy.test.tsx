/** Account creation follows optional-home policy without turning defaults into overrides. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Admin from '@/api/admin';
import { CreateUserDialog } from '@/features/admin/UserDialogs';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const calls = vi.hoisted(() => ({
  settings: vi.fn<() => Promise<ResolvedSetting[]>>(), create: vi.fn<typeof Admin.createUser>(),
}));
vi.mock('@/api/admin', async (original) => ({ ...await original<typeof Admin>(),
  systemSettingsQuery: { queryKey: ['creation-policy'], queryFn: calls.settings }, createUser: calls.create,
}));
const setting = (key: string, value: boolean): ResolvedSetting => ({ key, value,
  source: { scope: 'system', node: null }, own: null, writable: true, locked_by: null, parts: [] });
beforeEach(async () => {
  await setLocale('en'); vi.resetAllMocks();
  calls.settings.mockResolvedValue([setting('orgs.personal_enabled', true), setting('orgs.personal_closed', true), setting('orgs.personal_flat', true)]);
  calls.create.mockImplementation(() => new Promise(() => undefined));
});
afterEach(async () => { cleanup(); await setLocale('en'); });
const submit = () => screen.getByRole('button', { name: i18n.t('admin.users.create.submit') });
async function fill() {
  await userEvent.type(screen.getByLabelText(i18n.t('admin.users.create.username')), 'new-person');
  await userEvent.type(screen.getByLabelText(i18n.t('admin.users.create.name')), 'New person');
}
it.each(['en', 'zh-CN'] as const)('creates an account without a fabricated personal-org override in %s', async (locale) => {
  await setLocale(locale); calls.settings.mockResolvedValue([setting('orgs.personal_enabled', false)]);
  show(<CreateUserDialog open onClose={() => undefined} />);
  expect(await screen.findByText(i18n.t('admin.users.create.noOwnOrg'))).toBeVisible();
  expect(screen.queryByRole('group', { name: i18n.t('admin.users.create.ownOrg') })).not.toBeInTheDocument();
  await fill(); await userEvent.click(submit());
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(1));
  expect(calls.create.mock.calls[0]?.[0]).toEqual({ username: 'new-person', display_name: 'New person', is_system_admin: false });
});
it('leaves untouched personal defaults to the policy at commit time', async () => {
  show(<CreateUserDialog open onClose={() => undefined} />);
  expect(await screen.findByRole('checkbox', { name: i18n.t('admin.users.create.alone') })).toBeChecked();
  await fill(); await userEvent.click(submit());
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(1));
  expect(calls.create.mock.calls[0]?.[0]).not.toHaveProperty('personal_org');
});
it('sends only the personal restriction the administrator explicitly changed', async () => {
  show(<CreateUserDialog open onClose={() => undefined} />);
  await userEvent.click(await screen.findByRole('checkbox', { name: i18n.t('admin.users.create.alone') }));
  await fill(); await userEvent.click(submit());
  await waitFor(() => expect(calls.create).toHaveBeenCalledTimes(1));
  expect(calls.create.mock.calls[0]?.[0].personal_org).toEqual({ closed: false });
  expect(calls.create.mock.calls[0]?.[0].is_system_admin).toBe(false);
});
it('waits for the instance policy before submitting an account', async () => {
  calls.settings.mockImplementation(() => new Promise(() => undefined));
  show(<CreateUserDialog open onClose={() => undefined} />); await fill();
  expect(submit()).toBeDisabled();
  await userEvent.keyboard('{Enter}');
  expect(calls.create).not.toHaveBeenCalled();
});
