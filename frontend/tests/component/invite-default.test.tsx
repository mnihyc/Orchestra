/** The Invite form starts on the usable allowed form, with one creation request. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Settings from '@/api/settings';
import type * as Users from '@/api/users';
import { InviteDialog } from '@/features/org/Invite';
import type { ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { meta, org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';
const calls = vi.hoisted(() => ({ mail: false, forms: ['link', 'email'] as ('link' | 'email')[], make: vi.fn<typeof Users.createInvitation>() }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), metaQuery: { queryKey: ['invitation-meta'], queryFn: () => Promise.resolve(meta({ mail: calls.mail })) } }));
vi.mock('@/api/settings', async (original) => {
  const query = (id: string) => ({ queryKey: ['invitation-settings', id], queryFn: (): Promise<ResolvedSetting[]> => Promise.resolve([
    { key: 'invitations.forms', value: calls.forms, source: { scope: id === 'system' ? 'system' : 'org', node: null }, own: null, writable: true, locked_by: null, parts: [] },
  ]) });
  return { ...await original<typeof Settings>(), settingsQuery: query, targetSettingsQuery: (target: Parameters<typeof Settings.targetSettingsQuery>[0]) => query(target.kind === 'system' ? 'system' : target.id) };
});
vi.mock('@/api/users', async (original) => ({ ...await original<typeof Users>(), createInvitation: (...args: Parameters<typeof Users.createInvitation>) => calls.make(...args) }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); calls.mail = false; calls.forms = ['link', 'email']; calls.make.mockRejectedValue(new Error('The invitation could not be made.')); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
it.each(modes)('opens directly on Link when mail cannot leave, without an email field, in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const team = org('acme', 'acme', 'Acme', ADA);
  show(<InviteDialog target={{ kind: 'org', org: team }} open onClose={() => undefined} />);
  expect(await screen.findByRole('radio', { name: i18n.t('choiceCatalog.invitationForm.link.name') })).toBeChecked();
  expect(screen.queryByRole('textbox', { name: i18n.t('places.invite.email') })).not.toBeInTheDocument();
  expect(screen.getByRole('spinbutton', { name: i18n.t('places.invite.uses') })).toHaveValue(1);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('places.invite.submit') }));
  await waitFor(() => expect(calls.make).toHaveBeenCalledTimes(1));
  expect(calls.make.mock.calls[0]?.[0]).toEqual({ kind: 'org', org_id: 'acme', role: 'viewer', expires_in_hours: 168, max_uses: 1 });
});
it('keeps an email-only org on Email even without a mail transport', async () => {
  calls.forms = ['email'];
  show(<InviteDialog target={{ kind: 'org', org: org('acme', 'acme', 'Acme', ADA) }} open onClose={() => undefined} />);
  expect(await screen.findByRole('radio', { name: i18n.t('choiceCatalog.invitationForm.email.name') })).toBeChecked();
  expect(screen.getByRole('radio', { name: i18n.t('choiceCatalog.invitationForm.link.name') })).toBeDisabled();
  expect(screen.getByRole('textbox', { name: i18n.t('places.invite.email') })).toBeVisible();
  expect(calls.make).not.toHaveBeenCalled();
});

it.each(modes)('offers bounded system registration without an org or role in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  show(<InviteDialog target={{ kind: 'system' }} open onClose={() => undefined} />);
  expect(await screen.findByRole('radio', { name: i18n.t('choiceCatalog.invitationForm.link.name') })).toBeChecked();
  expect(screen.queryByRole('combobox', { name: i18n.t('places.invite.role') })).not.toBeInTheDocument();
  expect(screen.getByText(i18n.t('places.invite.becomesSystem'))).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: i18n.t('places.invite.submit') }));
  await waitFor(() => expect(calls.make).toHaveBeenCalledTimes(1));
  expect(calls.make.mock.calls[0]?.[0]).toEqual({ kind: 'system', expires_in_hours: 168, max_uses: 1 });
});
