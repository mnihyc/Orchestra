/** Registration, joining and optional personal homes keep their own effects. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Users from '@/api/users';
import type * as Router from '@tanstack/react-router';
import { Register } from '@/features/auth/Register';
import type { InvitationPreview, Meta } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, meta, org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  meta: vi.fn<() => Promise<Meta>>(), preview: vi.fn<() => Promise<InvitationPreview>>(),
  register: vi.fn<typeof Auth.register>(), accept: vi.fn<typeof Users.acceptInvitation>(),
  navigate: vi.fn<(...args: unknown[]) => Promise<void>>(),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(),
  metaQuery: { queryKey: ['registration-lifecycle-meta'], queryFn: calls.meta },
  nameAvailabilityQuery: (name: string) => ({ queryKey: ['registration-lifecycle-name', name], queryFn: () => Promise.resolve({ available: true }) }),
  register: calls.register,
}));
vi.mock('@/api/users', async (original) => ({ ...await original<typeof Users>(),
  invitationQuery: (name: string) => ({ queryKey: ['registration-lifecycle-invitation', name], queryFn: calls.preview }),
  acceptInvitation: calls.accept,
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  useRouter: () => ({ navigate: calls.navigate }),
  Link: ({ to, search, children, className }: Pick<AnchorHTMLAttributes<HTMLAnchorElement>, 'children' | 'className'> & { to: string; search?: { redirect?: string } }) =>
    <a href={search?.redirect === undefined ? to : `${to}?${new URLSearchParams({ redirect: search.redirect })}`} className={className}>{children}</a>,
}));
const previous = useAppearance.getState();
const invitation: InvitationPreview = { kind: 'system', org: null, what: null, role: null, permissions: [], inviter: ADA, email_bound: false, expires_at: '2027-01-01T00:00:00Z' };
beforeEach(() => {
  vi.clearAllMocks();
  calls.meta.mockResolvedValue(meta({ registration_mode: 'invite', identity_providers: [] }));
  calls.preview.mockResolvedValue(invitation);
  calls.navigate.mockResolvedValue(undefined);
  calls.register.mockResolvedValue({ signedIn: true, accepted: null,
    result: { access_token: 'not-a-credential', access_expires_at: '2027-01-01T00:00:00Z', user: me(14, 'new-person', 'New person', { personal_org_id: null }) } });
});
afterEach(async () => { cleanup(); useAppearance.setState(previous); await setLocale('en'); });
async function fill() {
  await userEvent.type(await screen.findByLabelText(i18n.t('auth.fields.username')), 'new-person');
  await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.displayName')), 'New person');
  await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'valid-passphrase-for-test');
}
for (const locale of ['en','zh-CN'] as const) for (const theme of ['light','dark'] as const) {
  it(`registers through a system invitation without joining or promising a personal org in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    show(<Register invitation="test-offer" />);
    await fill();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.register.submit') }));
    await waitFor(() => expect(calls.register).toHaveBeenCalledTimes(1));
    expect(calls.register.mock.calls[0]?.[0].accept_invitation).toBe(false);
    expect(calls.accept).not.toHaveBeenCalled();
    expect(await screen.findByText(i18n.t('auth.register.welcomeInvited'))).toBeVisible();
    expect(screen.queryByText(i18n.t('auth.register.ownOrgToo'))).not.toBeInTheDocument();
  });
  it(`uses one explicit register-and-join result in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    const team = org('acme','acme','Acme',ADA);
    calls.preview.mockResolvedValue({ ...invitation, kind: 'org', org: { name: 'Acme', slug: 'acme' }, role: 'viewer' });
    calls.register.mockResolvedValue({ signedIn: true, accepted: { org: team, node: null, chat: null },
      result: { access_token: 'not-a-credential', access_expires_at: '2027-01-01T00:00:00Z', user: me(14,'new-person','New person',{ personal_org_id: null }) } });
    show(<Register invitation="test-offer" />);
    await fill();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.register.submitJoin') }));
    await waitFor(() => expect(calls.register).toHaveBeenCalledTimes(1));
    expect(calls.register.mock.calls[0]?.[0].accept_invitation).toBe(true);
    expect(calls.accept).not.toHaveBeenCalled();
    expect(await screen.findByText(i18n.t('auth.register.welcomeOrg', { org: 'Acme' }))).toBeVisible();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.continue') }));
    expect(calls.navigate).toHaveBeenCalledWith({ to: '/org/$slug', params: { slug: 'acme' }, replace: true });
  });
}
it('keeps registration closed even when a valid invitation was supplied', async () => {
  calls.meta.mockResolvedValue(meta({ registration_mode: 'closed' }));
  show(<Register invitation="test-offer" />);
  expect(await screen.findByText(i18n.t('auth.register.closed'))).toBeVisible();
  expect(screen.queryByLabelText(i18n.t('auth.fields.username'))).not.toBeInTheDocument();
  expect(calls.register).not.toHaveBeenCalled();
});
it('offers only providers that can register and carries the explicit target intent', async () => {
  calls.meta.mockResolvedValue(meta({ registration_mode: 'invite', identity_providers: [
    { key: 'new-accounts', name: 'New accounts', auto_register: true },
    { key: 'existing-only', name: 'Existing accounts', auto_register: false },
  ] }));
  calls.preview.mockResolvedValue({ ...invitation, kind: 'org', org: { name: 'Acme', slug: 'acme' }, role: 'viewer' });
  show(<Register invitation="test-offer" />);
  const entry = await screen.findByRole('link', { name: i18n.t('auth.login.continueWith', { name: 'New accounts' }) });
  const query = new URL(entry.getAttribute('href') ?? '', 'http://test.local').searchParams;
  expect(query.get('accept_invitation')).toBe('true');
  expect(query.has('invitation')).toBe(true);
  expect(screen.queryByRole('link', { name: i18n.t('auth.login.continueWith', { name: 'Existing accounts' }) })).not.toBeInTheDocument();
});

it('keeps the invitation for someone who chooses to sign in to an existing account instead', async () => {
  calls.preview.mockResolvedValue({ ...invitation, kind: 'org', org: { name: 'Acme', slug: 'acme' }, role: 'viewer' });
  show(<Register invitation="test-offer" />);
  const entry = await screen.findByRole('link', { name: i18n.t('auth.links.signIn') });
  const next = new URL(entry.getAttribute('href') ?? '', 'http://test.local');
  expect(next.pathname).toBe('/login');
  expect(next.searchParams.get('redirect')).toBe('/invite/test-offer');
  expect(calls.register).not.toHaveBeenCalled();
  expect(calls.accept).not.toHaveBeenCalled();
});
