/** Lost invitation answers reuse their act; a new act follows a successful revoke. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Settings from '@/api/settings';
import type * as Users from '@/api/users';
import { InviteOutsider } from '@/features/access/Outsiders';
import { InviteDialog } from '@/features/org/Invite';
import { InvitationResult } from '@/features/org/InvitationResult';
import type { InvitationCreateResponse, ResolvedSetting } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { meta, org } from '../doubles/fixtures';
import { ADA, CATALOGUE, TOPIC } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ make: vi.fn<typeof Users.createInvitation>(), revoke: vi.fn<typeof Users.revokeInvitation>() }));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), metaQuery: { queryKey: ['replayed-invitation-meta'], queryFn: () => Promise.resolve(meta({ mail: false })) } }));
vi.mock('@/api/settings', async (load) => ({ ...await load<typeof Settings>(), targetSettingsQuery: () => ({ queryKey: ['replayed-invitation-settings'], queryFn: (): Promise<ResolvedSetting[]> => Promise.resolve([]) }) }));
vi.mock('@/api/users', async (load) => ({ ...await load<typeof Users>(), createInvitation: calls.make, revokeInvitation: calls.revoke }));

const ID = '00000000-0000-4000-8000-000000000111';
const replay: InvitationCreateResponse = { projection: 'receipt', invitation_id: ID, token: null, url: null, sent: false, already_created: true };
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
beforeEach(() => { calls.make.mockReset(); calls.revoke.mockReset(); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});

for (const kind of ['org', 'system', 'outsider'] as const) it.each(modes)(`${kind} keeps a lost answer and resets only after revoke in %s/%s`, async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  calls.make.mockRejectedValueOnce(new Error('The answer was lost')).mockResolvedValue(replay);
  calls.revoke.mockRejectedValueOnce(new Error('The revoke was refused')).mockResolvedValue(undefined);
  if (kind === 'outsider') show(<InviteOutsider target={{ kind: 'topic', id: TOPIC.id, name: 'Fix login' }} choice="full_view" catalogue={CATALOGUE} email="new@example.test" open onClose={() => undefined} />);
  else show(<InviteDialog target={kind === 'system' ? { kind: 'system' } : { kind: 'org', org: org('acme', 'acme', 'Acme', ADA) }} open onClose={() => undefined} />);
  const submitName = i18n.t(kind === 'outsider' ? 'access.outsiders.inviteSend' : 'places.invite.submit');
  const submit = await screen.findByRole('button', { name: submitName });
  await waitFor(() => expect(submit).toBeEnabled());
  await userEvent.click(submit);
  expect(await screen.findByRole('alert')).toBeVisible();
  await userEvent.click(submit);
  expect(await screen.findByText(i18n.t('places.invite.alreadyCreated'))).toBeVisible();
  expect(calls.make).toHaveBeenCalledTimes(2);
  expect(calls.make.mock.calls[0]?.[1]).toMatch(/^[0-9a-f-]{36}$/i);
  expect(calls.make.mock.calls[1]).toEqual(calls.make.mock.calls[0]);
  expect(screen.queryByRole('button', { name: i18n.t('places.invite.copy') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: i18n.t('access.outsiders.copyLink') })).not.toBeInTheDocument();
  const revoke = screen.getByRole('button', { name: i18n.t('places.invite.revokeAndAnother') });
  await userEvent.click(revoke);
  const confirm = screen.getByRole('button', { name: i18n.t('places.members.revokeConfirm') });
  await userEvent.click(confirm);
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(calls.revoke).toHaveBeenLastCalledWith(ID);
  expect(calls.make).toHaveBeenCalledTimes(2);
  expect(screen.queryByRole('button', { name: submitName })).not.toBeInTheDocument();
  await userEvent.click(confirm);
  const another = await screen.findByRole('button', { name: submitName });
  await userEvent.click(another);
  await waitFor(() => expect(calls.make).toHaveBeenCalledTimes(3));
  expect(calls.make.mock.calls[2]?.[0]).toEqual(calls.make.mock.calls[0]?.[0]);
  expect(calls.make.mock.calls[2]?.[1]).not.toBe(calls.make.mock.calls[0]?.[1]);
});

it.each(['en', 'zh-CN'] as const)('shows a real first capability in its declared receipt in %s', async (locale) => {
  await setLocale(locale);
  show(<InvitationResult reply={{ ...replay, already_created: false, token: 'first-only', url: 'https://example.test/invite/first-only' }} onAnother={() => undefined} linkLabel={i18n.t('places.invite.link')} copyLabel={i18n.t('places.invite.copy')} />);
  expect(screen.getByRole('textbox', { name: i18n.t('places.invite.link') })).toHaveValue('https://example.test/invite/first-only');
  expect(screen.getByRole('button', { name: i18n.t('places.invite.copy') })).toBeVisible();
  expect(calls.revoke).not.toHaveBeenCalled();
});
