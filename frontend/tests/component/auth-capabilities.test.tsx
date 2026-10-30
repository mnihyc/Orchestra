/** Auth offers only actions this system can complete and identifies the account after its first proof. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { AnchorHTMLAttributes } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Auth from '@/api/auth';
import type * as Router from '@tanstack/react-router';
import { ApiError } from '@/api/client';
import { AuthFrame } from '@/features/auth/Frame';
import { Login } from '@/features/auth/Login';
import { Recovery } from '@/features/auth/Recovery';
import { Register } from '@/features/auth/Register';
import { ResetPassword } from '@/features/auth/ResetPassword';
import { VerifyEmail } from '@/features/auth/VerifyEmail';
import type { Meta } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { meta } from '../doubles/fixtures';
import { show } from './support';
import { retiredCopy } from './retired-copy';

const calls = vi.hoisted(() => ({
  meta: vi.fn<() => Promise<Meta>>(), signIn: vi.fn<typeof Auth.signIn>(),
  askCode: vi.fn<typeof Auth.askEmailCode>(), recover: vi.fn<typeof Auth.askRecovery>(), resend: vi.fn<typeof Auth.resendConfirmation>(),
  readRecovery: vi.fn<() => Promise<boolean>>(), confirmRecovery:vi.fn<typeof Auth.confirmRecovery>(), navigate: vi.fn<(...args: unknown[]) => Promise<void>>(),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(),
  metaQuery: { queryKey: ['auth-capabilities-meta'], queryFn: calls.meta },
  recoveryQuery: (token: string) => ({ queryKey: ['auth-capabilities-reset', token], queryFn: calls.readRecovery }),
  signIn: calls.signIn, askEmailCode: calls.askCode, askRecovery: calls.recover, resendConfirmation: calls.resend,
  confirmRecovery:calls.confirmRecovery,
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(),
  useRouter: () => ({ navigate: calls.navigate, history: { replace: vi.fn() } }),
  Link: ({ to, children, className }: Pick<AnchorHTMLAttributes<HTMLAnchorElement>, 'children' | 'className'> & { to: string }) => <a href={to} className={className}>{children}</a>,
}));

const appearance = useAppearance.getState();
const themeBefore = document.documentElement.getAttribute('data-theme');
function expired() { return new ApiError({ type: 'orch:error:gone', code: 'gone', status: 410, title: 'Link expired', detail: 'The link ended.', request_id: 'link-request' }, null); }
beforeEach(() => {
  vi.clearAllMocks();
  calls.meta.mockResolvedValue(meta({ name: 'Workshop', mail: false, email_code_login: true, registration_mode: 'invite' }));
  calls.readRecovery.mockRejectedValue(expired());
  calls.confirmRecovery.mockResolvedValue(undefined);
  calls.recover.mockResolvedValue(undefined); calls.resend.mockResolvedValue(undefined); calls.askCode.mockResolvedValue(undefined);
  calls.navigate.mockResolvedValue(undefined);
});
afterEach(async () => {
  cleanup(); useAppearance.setState(appearance); await setLocale('en');
  if (themeBefore === null) document.documentElement.removeAttribute('data-theme');
  else document.documentElement.dataset.theme = themeBefore;
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`hides every unavailable mail act at sign-in and on expired links in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const login = show(<AuthFrame><Login onward={null} callbackError={null} /></AuthFrame>);
    await screen.findByLabelText(i18n.t('auth.fields.login'));
    expect(screen.queryByRole('link', { name: i18n.t('auth.links.forgot') })).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: i18n.t('auth.links.register') })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: i18n.t('auth.login.mailCode') })).not.toBeInTheDocument();
    expect(screen.getByText(i18n.t('auth.login.byInvitation'))).toBeVisible();
    calls.signIn.mockRejectedValueOnce(new ApiError({ type: 'orch:error:auth.email_unverified', code: 'auth.email_unverified', status: 403, title: 'Email not confirmed', detail: 'Confirm your email.', request_id: 'confirmation-request' }, null));
    await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.login')), 'reader');
    await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'correct-secret');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.login.submit') }));
    expect(await screen.findByText(i18n.t('auth.verify.noMail'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('auth.verify.resend') })).not.toBeInTheDocument();
    login.unmount();
    const reset = show(<AuthFrame><ResetPassword token="expired" /></AuthFrame>);
    expect(await screen.findByText(i18n.t('auth.reset.spent'))).toBeVisible();
    expect(screen.queryByRole('link', { name: i18n.t('auth.reset.askAgain') })).not.toBeInTheDocument();
    expect(await screen.findByText(i18n.t('auth.recover.noMail'))).toBeVisible();
    reset.unmount();
    const verify = show(<AuthFrame><VerifyEmail outcome={{ confirmed: false, error: expired() }} onRetry={vi.fn()} /></AuthFrame>);
    expect(await screen.findByText(i18n.t('auth.verify.noMail'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('auth.verify.resend') })).not.toBeInTheDocument();
    verify.unmount();
    show(<AuthFrame><Recovery /></AuthFrame>);
    expect(await screen.findByText(i18n.t('auth.recover.noMail'))).toBeVisible();
    expect(screen.queryByRole('button', { name: i18n.t('auth.recover.submit') })).not.toBeInTheDocument();
    expect(screen.queryByLabelText(i18n.t('auth.fields.email'))).not.toBeInTheDocument();
    expect(calls.recover).not.toHaveBeenCalled(); expect(calls.resend).not.toHaveBeenCalled(); expect(calls.askCode).not.toHaveBeenCalled();
  });

  it(`retains mail-capable recovery and confirmation paths in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    calls.meta.mockResolvedValue(meta({ name: 'Workshop', mail: true, registration_mode: 'open' }));
    const recovery = show(<AuthFrame><Recovery /></AuthFrame>);
    await userEvent.type(await screen.findByLabelText(i18n.t('auth.fields.email')), 'reader@example.test');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.recover.submit') }));
    await waitFor(() => expect(calls.recover.mock.lastCall?.[0]).toBe('reader@example.test'));
    expect(calls.recover).toHaveBeenCalledTimes(1);
    expect(await screen.findByText(i18n.t('auth.recover.sent'))).toBeVisible();
    recovery.unmount();
    const reset = show(<AuthFrame><ResetPassword token="expired" /></AuthFrame>);
    const again = await screen.findByRole('link', { name: i18n.t('auth.reset.askAgain') });
    expect(again).toHaveAttribute('href', '/recover');
    reset.unmount();
    show(<AuthFrame><VerifyEmail outcome={{ confirmed: false, error: expired() }} onRetry={vi.fn()} /></AuthFrame>);
    await userEvent.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.verify.resend') }));
    await waitFor(() => expect(calls.resend.mock.lastCall?.[0]).toBe('reader'));
    expect(calls.resend).toHaveBeenCalledTimes(1);
  });

  it(`returns direct invitation-only registration to sign-in in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    const view = show(<AuthFrame><Register invitation={null} /></AuthFrame>);
    await waitFor(() => expect(calls.navigate).toHaveBeenCalledWith({ to: '/login', replace: true }));
    expect(screen.queryByRole('button', { name: i18n.t('auth.register.submit') })).not.toBeInTheDocument();
    expect(screen.queryByRole('heading', { name: i18n.t('auth.register.title') })).not.toBeInTheDocument();
    view.rerender(<AuthFrame><Login onward={null} callbackError={null} /></AuthFrame>);
    expect(await screen.findByText(i18n.t('auth.login.byInvitation'))).toBeVisible();
  });

  it(`accepts a valid administrator-delivered reset link without mail in ${locale}/${theme}`,async()=>{
    await setLocale(locale);useAppearance.setState({locale,theme});document.documentElement.dataset.theme=theme;
    calls.readRecovery.mockResolvedValueOnce(true);
    show(<AuthFrame><ResetPassword token="admin-delivered-link" /></AuthFrame>);
    await userEvent.type(await screen.findByLabelText(i18n.t('auth.fields.newPassword')),'new-valid-password-5678');
    await userEvent.click(screen.getByRole('button',{name:i18n.t('auth.reset.submit')}));
    await waitFor(()=>expect(calls.confirmRecovery.mock.lastCall?.[0]).toEqual({token:'admin-delivered-link',password:'new-valid-password-5678'}));
    expect(await screen.findByText(i18n.t('auth.reset.done'))).toBeVisible();
    expect(calls.recover).not.toHaveBeenCalled();expect(calls.resend).not.toHaveBeenCalled();
  });

  it(`names the verified first-proof account while keeping the recovery-code path in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme }); document.documentElement.dataset.theme = theme;
    calls.signIn.mockRejectedValueOnce(new ApiError({ type: 'orch:error:auth.second_step_required', code: 'auth.second_step_required', status: 403,
      title: 'Second step', detail: 'Another proof is required.', request_id: 'proof-request', methods: ['totp', 'recovery_code', 'email_code'] }, null));
    const view = show(<AuthFrame><Login onward="/thread/original" callbackError={null} /></AuthFrame>);
    await userEvent.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'grace');
    await userEvent.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'first-proof-secret');
    await userEvent.click(screen.getByRole('button', { name: i18n.t('auth.login.submit') }));
    expect(await screen.findByText(i18n.t('auth.second.account', { account: 'grace' }))).toBeVisible();
    expect(screen.queryByText(retiredCopy(locale, 'auth.second.lead.totp'))).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('auth.second.use.recovery_code') })).toBeEnabled();
    expect(screen.queryByRole('button', { name: i18n.t('auth.second.use.email_code') })).not.toBeInTheDocument();
    expect(view.container).not.toHaveTextContent('first-proof-secret');
    expect(Array.from(view.container.querySelectorAll<HTMLInputElement>('input'),(input)=>input.value)).not.toContain('first-proof-secret');
    expect(calls.askCode).not.toHaveBeenCalled();
  });
}
