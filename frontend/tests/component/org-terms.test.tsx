/** One displayed consent binds Join, Register and SSO to the current native terms hash. */
import { act, cleanup, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import { ApiError, client } from '@/api/client';
import type * as Users from '@/api/users';
import { Invitation } from '@/features/auth/Invitation';
import { Register } from '@/features/auth/Register';
import { useInvitationTerms, type InvitationTerms } from '@/features/auth/Terms';
import type { InvitationPreview, Meta, Problem } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, meta, org } from '../doubles/fixtures';
import { ADA } from '../unit/general-access-fixtures';
import { show } from './support';

const FIRST = 'a'.repeat(64), NEXT = 'b'.repeat(64);
const terms: InvitationTerms = { text: '# Team terms\n\nKeep client data here', hash: FIRST };
const invitation = (current: InvitationTerms | null = terms): InvitationPreview & { terms: InvitationTerms | null } => ({
  kind: 'org', org: { name: 'Acme', slug: 'acme' }, what: null, role: 'viewer', permissions: [],
  inviter: ADA, email_bound: false, expires_at: '2027-01-01T00:00:00Z', terms: current,
});
const calls = vi.hoisted(() => ({
  preview: vi.fn<() => Promise<InvitationPreview>>(), accept: vi.fn<typeof Users.acceptInvitation>(),
  register: vi.fn<typeof Auth.register>(), meta: vi.fn<() => Promise<Meta>>(), navigate: vi.fn<() => Promise<void>>(),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(),
  metaQuery: { queryKey: ['terms-meta'], queryFn: calls.meta },
  meQuery: { queryKey: ['terms-me'], queryFn: () => Promise.resolve(me(1, 'reader', 'Reader')) },
  nameAvailabilityQuery: (name: string) => ({ queryKey: ['terms-name', name], queryFn: () => Promise.resolve({ available: true }) }),
  register: calls.register,
}));
vi.mock('@/api/users', async (original) => ({ ...await original<typeof Users>(),
  invitationQuery: (token: string) => ({ queryKey: ['terms-invitation', token], queryFn: calls.preview }), acceptInvitation: calls.accept,
}));
vi.mock('@/features/navigation/placeLabels', async (loadOriginal) => ({ ...await loadOriginal<object>(), usePlaceLabels: () => new Map() }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), useRouter: () => ({ navigate: calls.navigate }),
  Link: ({ to, search, children, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; search?: Record<string, string | undefined> }) => {
    const query = new URLSearchParams(Object.entries(search ?? {}).filter((pair): pair is [string, string] => pair[1] !== undefined));
    return <a {...props} href={`${to}${query.size === 0 ? '' : `?${query.toString()}`}`}>{children}</a>;
  },
}));
const previous = client.session.getState(), appearance = useAppearance.getState();
function changed(): ApiError {
  const problem: Problem = { type: 'orch:error:invitation.terms_not_accepted', code: 'invitation.terms_not_accepted', status: 409, title: 'Terms changed', detail: '', request_id: 'terms-request',
    terms: { text: '# New terms\n\nNew team rule', hash: NEXT } };
  return new ApiError(problem, null);
}
beforeEach(() => {
  vi.clearAllMocks(); client.session.setState({ status: 'signed_in', userId: 'reader', readerVersion: 702 });
  calls.preview.mockResolvedValue(invitation()); calls.accept.mockResolvedValue({ org: org('org', 'acme', 'Acme', ADA), node: null, chat: null });
  calls.register.mockResolvedValue({ signedIn: false }); calls.navigate.mockResolvedValue(undefined);
  calls.meta.mockResolvedValue(meta({ name: 'Workshop', registration_mode: 'invite', mail: false, identity_providers: [{ key: 'company', name: 'Company SSO', auto_register: true }] }));
});
afterEach(async () => { cleanup(); client.session.setState(previous); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`requires one explicit checkbox and sends the displayed hash in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme });
    show(<Invitation token="invite" system="Workshop" />);
    const checkbox = await screen.findByRole('checkbox', { name: i18n.t('orgTerms.accept', { org: 'Acme' }) });
    expect(screen.getAllByRole('checkbox')).toHaveLength(1);
    expect(screen.getByRole('region', { name: i18n.t('orgTerms.title') })).toHaveTextContent('Keep client data here');
    expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeDisabled();
    expect(calls.accept).not.toHaveBeenCalled();
    await userEvent.click(checkbox);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.invite.accept') }));
    await waitFor(() => expect(calls.accept).toHaveBeenCalledWith('invite', FIRST));
  });
}

it.each(['en', 'zh-CN'] as const)('refreshes changed terms and requires a new acceptance in %s', async (locale) => {
  await setLocale(locale); calls.accept.mockRejectedValueOnce(changed());
  show(<Invitation token="invite" system="Workshop" />);
  await userEvent.click(await screen.findByRole('checkbox'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.invite.accept') }));
  expect(await screen.findByText('New team rule')).toBeVisible();
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeDisabled();
  await userEvent.click(screen.getByRole('checkbox'));
  await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.invite.accept') }));
  await waitFor(() => expect(calls.accept).toHaveBeenLastCalledWith('invite', NEXT));
});

it('carries the confirmed hash from the invitation to registration', async () => {
  client.session.setState({ status: 'signed_out', userId: null, readerVersion: 703 });
  show(<Invitation token="invite" system="Workshop" />);
  await userEvent.click(await screen.findByRole('checkbox'));
  const link = screen.getByRole('link', { name: i18n.t('auth.invite.register') });
  const address = new URL(link.getAttribute('href') ?? '', window.location.origin);
  expect(address.searchParams.get('invitation')).toBe('invite');
  expect(address.searchParams.get('accepted_terms_hash')).toBe(FIRST);
});

async function fillRegistration() {
  const user = userEvent.setup();
  await user.type(await screen.findByRole('textbox', { name: i18n.t('auth.fields.username') }), 'new-reader');
  await user.type(screen.getByRole('textbox', { name: i18n.t('auth.fields.displayName') }), 'New reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'a-long-private-password');
}

it.each(['en', 'zh-CN'] as const)('uses the same consent for registration and SSO and preserves a refused form in %s', async (locale) => {
  await setLocale(locale); client.session.setState({ status: 'signed_out', userId: null, readerVersion: 703 });
  calls.register.mockRejectedValueOnce(changed());
  show(<Register invitation="invite" />);
  await fillRegistration();
  const submit = screen.getByRole('button', { name: i18n.t('auth.register.submitJoin') });
  const sso = screen.getByRole('link', { name: i18n.t('auth.login.continueWith', { name: 'Company SSO' }) });
  expect(submit).toBeDisabled(); expect(sso).toHaveAttribute('aria-disabled', 'true');
  await userEvent.click(screen.getByRole('checkbox'));
  expect(new URL(sso.getAttribute('href') ?? '', window.location.origin).searchParams.get('accepted_terms_hash')).toBe(FIRST);
  await userEvent.click(submit);
  await waitFor(() => expect(calls.register).toHaveBeenCalledWith(expect.objectContaining({ invitation: 'invite', accept_invitation: true, accepted_terms_hash: FIRST }), expect.stringMatching(/^[0-9a-f-]{36}$/u)));
  expect(await screen.findByText('New team rule')).toBeVisible();
  expect(screen.getByRole('checkbox')).not.toBeChecked(); expect(submit).toBeDisabled();
  expect(screen.getByRole('textbox', { name: i18n.t('auth.fields.username') })).toHaveValue('new-reader');
  expect(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true })).toHaveValue('a-long-private-password');
  await userEvent.click(screen.getByRole('checkbox'));
  expect(new URL(sso.getAttribute('href') ?? '', window.location.origin).searchParams.get('accepted_terms_hash')).toBe(NEXT);
});

it('rechecks public terms before SSO without creating a start for changed text', async () => {
  client.session.setState({ status: 'signed_out', userId: null, readerVersion: 703 });
  show(<Register invitation="invite" acceptedTermsHash={FIRST} />);
  expect(await screen.findByRole('checkbox')).toBeChecked();
  calls.preview.mockResolvedValue(invitation({ text: '# New terms', hash: NEXT }));
  await userEvent.click(screen.getByRole('link', { name: i18n.t('auth.login.continueWith', { name: 'Company SSO' }) }));
  await screen.findByRole('heading', { name: 'New terms' });
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('link', { name: i18n.t('auth.login.continueWith', { name: 'Company SSO' }) })).toHaveAttribute('aria-disabled', 'true');
  expect(calls.register).not.toHaveBeenCalled();
});

it('keeps no-terms invitations on the existing acceptance path', async () => {
  calls.preview.mockResolvedValue(invitation(null));
  show(<Invitation token="invite" system="Workshop" />);
  await userEvent.click(await screen.findByRole('button', { name: i18n.t('auth.invite.accept') }));
  expect(screen.queryByRole('checkbox')).toBeNull();
  await waitFor(() => expect(calls.accept).toHaveBeenCalledWith('invite'));
});

it('never converts malformed offered terms into no terms', async () => {
  const malformed = { ...invitation(), terms: { text: 'Not a complete offer', hash: '' } };
  calls.preview.mockResolvedValue(malformed);
  show(<Invitation token="invite" system="Workshop" />);
  expect(await screen.findByText(i18n.t('orgTerms.unavailable'))).toBeVisible();
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeDisabled();
  expect(calls.accept).not.toHaveBeenCalled();
});


it.each(['node', 'chat'] as const)('requires the same organization terms for a %s guest invitation', async (kind) => {
  const guest = { ...invitation(), kind, what: { kind: kind === 'node' ? 'topic' as const : 'chat' as const, name: 'Review' } };
  calls.preview.mockResolvedValue(guest);
  show(<Invitation token="guest" system="Workshop" />);
  expect(await screen.findByRole('checkbox', { name: i18n.t('orgTerms.accept', { org: 'Acme' }) })).not.toBeChecked();
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeDisabled();
});

it('retires consent when the reader changes', async () => {
  show(<Invitation token="invite" system="Workshop" />);
  await userEvent.click(await screen.findByRole('checkbox'));
  expect(screen.getByRole('checkbox')).toBeChecked();
  act(() => client.session.setState({ userId: 'other-reader', readerVersion: 704 }));
  expect(screen.getByRole('checkbox')).not.toBeChecked();
  expect(screen.getByRole('button', { name: i18n.t('auth.invite.accept') })).toBeDisabled();
  expect(calls.accept).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('does not let a prior committed reader review or replace the current terms in %s', async (locale) => {
  await setLocale(locale);
  const { result } = renderHook(() => useInvitationTerms('invite', invitation()));
  act(() => result.current.setChecked(true));
  expect(result.current.ready).toBe(true);
  const oldReview = result.current.review, oldRefusal = result.current.handleError;
  act(() => client.session.setState({ userId: 'new-reader', readerVersion: 704 }));
  expect(result.current.checked).toBe(false);
  act(() => {
    expect(oldReview(invitation())).toBe(false);
    expect(oldRefusal(changed())).toBe(true);
  });
  expect(result.current.terms).toEqual(terms);
  expect(result.current.ready).toBe(false);
  expect(calls.accept).not.toHaveBeenCalled();
});
