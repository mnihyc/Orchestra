/** An invitation keeps the current account and explains a failed attempt to switch accounts. */
import type { AnchorHTMLAttributes } from 'react';
import type * as Router from '@tanstack/react-router';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import { client } from '@/api/client';
import type * as Users from '@/api/users';
import { Invitation } from '@/features/auth/Invitation';
import type { InvitationPreview, Me } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { me } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { deferred } from './deferred';
import { show } from './support';

const calls = vi.hoisted(() => ({
  me: vi.fn<() => Promise<Me>>(), preview: vi.fn<() => Promise<InvitationPreview>>(),
  signOut: vi.fn<typeof Auth.signOut>(), accept: vi.fn<typeof Users.acceptInvitation>(),
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(),
  meQuery: { queryKey: ['invitation-signout-me'], queryFn: calls.me }, signOut: calls.signOut,
}));
vi.mock('@/api/users', async (load) => ({ ...await load<typeof Users>(),
  invitationQuery: (token: string) => ({ queryKey: ['invitation-signout', token], queryFn: calls.preview }), acceptInvitation: calls.accept,
}));
vi.mock('@/features/navigation/placeLabels', () => ({ usePlaceLabels: () => new Map() }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useRouter: () => ({ navigate: vi.fn() }),
  Link: ({ to, children, className }: Pick<AnchorHTMLAttributes<HTMLAnchorElement>, 'children' | 'className'> & { to: string }) => <a href={to} className={className}>{children}</a>,
}));
const invitation: InvitationPreview = { kind: 'system', org: null, what: null, role: null, permissions: [], inviter: ADA, email_bound: false, expires_at: '2027-01-01T00:00:00Z' };
const previous = useAppearance.getState();
const previousSession = client.session.getState();
beforeEach(() => {
  client.session.setState({ status: 'signed_in', userId: 'reader', readerVersion: 201 });
  calls.me.mockReset().mockResolvedValue(me(1, 'reader', 'Reader'));
  calls.preview.mockReset().mockResolvedValue(invitation);
  calls.accept.mockReset();
  calls.signOut.mockReset();
});
afterEach(async () => { cleanup(); client.session.setState(previousSession); useAppearance.setState(previous); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`shows a failed account switch and lets the person retry in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    calls.signOut.mockRejectedValueOnce(new NetworkError('offline'));
    calls.signOut.mockImplementationOnce(() => { client.session.setState({ status: 'signed_out', userId: null, readerVersion: 202 }); return Promise.resolve(); });
    show(<Invitation token="valid" system="Workshop" />);
    const user = userEvent.setup();
    await screen.findByText(i18n.t('auth.invite.signedInAs', { name: 'Reader' }), { exact: false });
    await user.click(screen.getByRole('button', { name: i18n.t('auth.invite.notYou') }));
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('network.offline'));
    expect(screen.getByRole('button', { name: i18n.t('auth.invite.notYou') })).toBeEnabled();
    expect(client.session.getState().userId).toBe('reader');
    expect(calls.accept).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: i18n.t('auth.invite.notYou') }));
    await waitFor(() => expect(client.session.getState().status).toBe('signed_out'));
    expect(await screen.findByRole('link', { name: i18n.t('auth.links.signIn') })).toBeVisible();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
}

it('does not accept an invitation while the account switch is pending', async () => {
  await setLocale('en');
  const pending = deferred<void>();
  calls.signOut.mockReturnValueOnce(pending.promise);
  show(<Invitation token="valid" system="Workshop" />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('auth.invite.notYou') }));
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.notYou') })).toBeDisabled();
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeDisabled();
  expect(calls.accept).not.toHaveBeenCalled();
  await act(async () => { pending.reject(new NetworkError('offline')); await Promise.resolve(); });
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeEnabled();
});
