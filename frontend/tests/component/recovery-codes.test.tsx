/** A saved second factor, issued once and replaced with fresh proof; plaintext stays out of caches. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import type * as Auth from '@/api/auth';
import type * as Users from '@/api/users';
import { beforeEach, expect, it, vi } from 'vitest';

import { ApiError } from '@/api/client';
import { answerChanges } from '@/api/actAnswers';
import { freshProofAddress } from '@/features/auth/freshProof';
import { Login } from '@/features/auth/Login';
import { SecondStepGate } from '@/features/auth/SecondStepGate';
import { ReplaceRecoveryCodes, SavedRecoveryCodes } from '@/features/settings/personal/RecoveryCodes';
import { Security } from '@/features/settings/personal/Security';
import type { Me, Meta, RecoveryCodeStatus } from '@/generated/api-types';
import { setLocale } from '@/i18n';

import { me, meta } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  me: vi.fn<() => Promise<Me>>(), meta: vi.fn<() => Promise<Meta>>(),
  status: vi.fn<() => Promise<RecoveryCodeStatus>>(),
  signIn: vi.fn<typeof Auth.signIn>(), verify: vi.fn<typeof Users.verifySecondStep>(),
  setup: vi.fn<typeof Users.setUpAuthenticator>(), enable: vi.fn<typeof Users.enableAuthenticator>(),
  replace: vi.fn<typeof Users.regenerateRecoveryCodes>(),
  navigate: vi.fn<(...args: unknown[]) => Promise<void>>(), history: vi.fn(),
}));
vi.mock('@/api/auth', async (original) => ({ ...(await original<typeof Auth>()),
  meQuery: { queryKey: ['me'], queryFn: calls.me }, metaQuery: { queryKey: ['meta'], queryFn: calls.meta }, signIn: calls.signIn,
}));
vi.mock('@/api/users', async (original) => ({ ...(await original<typeof Users>()),
  recoveryCodesQuery: { queryKey: ['recovery-codes'], queryFn: calls.status },
  sessionsQuery: { queryKey: ['sessions'], queryFn: () => Promise.resolve([]) },
  regenerateRecoveryCodes: calls.replace, verifySecondStep: calls.verify,
  setUpAuthenticator: calls.setup, enableAuthenticator: calls.enable,
}));
vi.mock('@tanstack/react-router', async (original) => ({ ...(await original<typeof Router>()),
  useRouter: () => ({ navigate: calls.navigate, history: { replace: calls.history } }),
  Link: ({ to, children }: { to: string; children: ReactNode }) => <a href={to}>{children}</a>,
}));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));
vi.mock('@/features/auth/Frame', () => ({ AuthCard: ({ title, children, foot }: { title: string; children: ReactNode; foot: ReactNode }) => <section><h1>{title}</h1>{children}{foot}</section> }));

const CODES = ['0123-4567-89AB-CDEF-GHJK', '2345-6789-ABCD-EFGH-JKMN'];
const status = (more: Partial<RecoveryCodeStatus> = {}): RecoveryCodeStatus => ({ remaining: 2, created_at: '2026-10-15T12:00:00.000Z', methods: ['totp', 'recovery_code'], recent_recovery: false, ...more });
const missing = () => new ApiError({ type: 'orch:error:auth.second_step_required', code: 'auth.second_step_required', title: 'Second step', detail: 'one more proof', status: 401, request_id: 'r', methods: ['totp', 'recovery_code'] }, null);

beforeEach(async () => {
  vi.clearAllMocks();
  sessionStorage.clear();
  window.history.replaceState(null, '', '/settings/security');
  await setLocale('en');
  calls.me.mockResolvedValue(me(1, 'ada', 'Ada'));
  calls.meta.mockResolvedValue(meta());
  calls.status.mockResolvedValue(status());
  calls.replace.mockResolvedValue({ codes: CODES, status: status() });
  calls.navigate.mockResolvedValue(undefined);
});

it.each(['en', 'zh-CN'] as const)('signs in with the saved second factor in %s and preserves the original destination', async (locale) => {
  await setLocale(locale);
  calls.signIn.mockRejectedValueOnce(missing()).mockResolvedValueOnce({ access_token: 'session', access_expires_at: '2026-10-15T12:00:00.000Z', user: me(1, 'ada', 'Ada') });
  show(<Login onward="/thread/original" callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(locale === 'en' ? 'Username or email address' : '用户名或邮箱地址'), 'ada');
  await user.type(screen.getByLabelText(locale === 'en' ? 'Password' : '密码', { exact: true }), 'correct horse battery');
  await user.click(screen.getByRole('button', { name: locale === 'en' ? 'Sign in' : '登录' }));
  await user.click(await screen.findByRole('button', { name: locale === 'en' ? 'Use a recovery code' : '使用恢复码' }));
  const code = screen.getByLabelText(locale === 'en' ? 'Recovery code' : '恢复码', { exact: true });
  await user.type(code, CODES[0] ?? '');
  await user.keyboard('{Enter}');
  await waitFor(() => expect(calls.signIn).toHaveBeenLastCalledWith({ login: 'ada', password: 'correct horse battery', recovery_code: CODES[0] }));
  await waitFor(() => expect(calls.history).toHaveBeenCalledWith('/thread/original'));
});

it('an existing browser offers recovery and app choices and sends only the chosen proof', async () => {
  const done = vi.fn();
  calls.verify.mockResolvedValue(me(1, 'ada', 'Ada'));
  show(<SecondStepGate error={missing()} onDone={done} />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Use a recovery code' }));
  await user.type(screen.getByLabelText('Recovery code', { exact: true }), CODES[0] ?? '');
  await user.click(screen.getByRole('button', { name: 'Verify' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  expect(calls.verify).toHaveBeenCalledExactlyOnceWith({ recovery_code: CODES[0] });
});

it('activation shows its batch once and does not read recovery metadata on page load or after issuance', async () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  calls.setup.mockResolvedValue({ secret: 'JBSWY3DPEHPK3PXP', otpauth_url: 'otpauth://totp/Orch:ada?secret=JBSWY3DPEHPK3PXP' });
  calls.enable.mockResolvedValue({ me: me(1, 'ada', 'Ada', { totp_enabled: true }), recovery_codes: { codes: CODES, status: status() } });
  show(<QueryClientProvider client={client}><Security /></QueryClientProvider>);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('switch', { name: 'Authenticator app' }));
  const setup = screen.getByRole('dialog', { name: 'Set up an authenticator app' });
  await user.type(within(setup).getByLabelText('Your password'), 'correct horse battery');
  await user.click(within(setup).getByRole('button', { name: 'Continue' }));
  await user.type(await within(setup).findByLabelText('Code from the app'), '135790');
  await user.click(within(setup).getByRole('button', { name: 'Switch on' }));
  const saved = await screen.findByRole('dialog', { name: 'Save your recovery codes' });
  expect(within(saved).getByText(CODES[0] ?? '')).toBeVisible();
  expect(calls.status).not.toHaveBeenCalled();
  expect(JSON.stringify(client.getQueryCache().getAll().map((query) => query.state.data))).not.toContain(CODES[0]);
  expect(JSON.stringify(client.getMutationCache().getAll().map((mutation) => mutation.state.data))).not.toContain(CODES[0]);
  await user.click(within(saved).getByRole('button', { name: 'Close' }));
  expect(screen.queryByText(CODES[0] ?? '')).not.toBeInTheDocument();
});

it('shows only safe metadata in the cache projection for setup and replacement', () => {
  const account = me(1, 'ada', 'Ada');
  const common = { input: undefined, method: 'POST' as const };
  const setup = answerChanges({ ...common, path: '/me/totp/enable', answer: { me: account, recovery_codes: { codes: CODES, status: status() } } }, () => '', [{ key: ['me'] }]);
  const replaced = answerChanges({ ...common, path: '/me/recovery-codes', answer: { codes: CODES, status: status() } }, () => '', [{ key: ['recovery-codes'] }]);
  expect(setup).toEqual([{ value: { key: ['me'], data: account } }, { value: { key: ['recovery-codes'], data: status() } }]);
  expect(JSON.stringify([...setup, ...replaced])).not.toContain(CODES[0]);
});

it('replacement asks for fresh first and second proof and sends only one code type', async () => {
  const issued = vi.fn();
  show(<ReplaceRecoveryCodes me={me(1, 'ada', 'Ada', { totp_enabled: true })} returned={null} onClose={vi.fn()} onIssued={issued} />);
  const user = userEvent.setup();
  await screen.findByLabelText('Your password');
  const dialog = screen.getByRole('dialog', { name: 'Replace recovery codes?' });
  await user.click(within(dialog).getByRole('button', { name: 'Generate new codes' }));
  expect(calls.replace).not.toHaveBeenCalled();
  await user.type(within(dialog).getByLabelText('Your password'), 'correct horse battery');
  await user.click(within(dialog).getByRole('button', { name: 'Use a recovery code' }));
  await user.type(within(dialog).getByLabelText('Recovery code', { exact: true }), CODES[0] ?? '');
  await user.click(within(dialog).getByRole('button', { name: 'Generate new codes' }));
  await waitFor(() => expect(issued).toHaveBeenCalledWith(CODES));
  expect(calls.replace).toHaveBeenCalledExactlyOnceWith({ password: 'correct horse battery', recovery_code: CODES[0] });
});

it('the last-code browser can replace once with a fresh first proof and expiry restores the second-code field', async () => {
  calls.status.mockResolvedValue(status({ remaining: 0, methods: ['totp'], recent_recovery: true }));
  calls.replace.mockRejectedValueOnce(missing());
  show(<ReplaceRecoveryCodes me={me(1, 'ada', 'Ada', { totp_enabled: true })} returned={null} onClose={vi.fn()} onIssued={vi.fn()} />);
  const user = userEvent.setup();
  const password = await screen.findByLabelText('Your password');
  expect(screen.queryByLabelText('Code from your authenticator app')).not.toBeInTheDocument();
  await user.type(password, 'correct horse battery');
  await user.click(screen.getByRole('button', { name: 'Generate new codes' }));
  await waitFor(() => expect(calls.replace).toHaveBeenCalledWith({ password: 'correct horse battery' }));
  expect(await screen.findByLabelText('Code from your authenticator app')).toBeVisible();
});

it('a passwordless replacement uses the fresh provider proof scoped to this act', async () => {
  show(<ReplaceRecoveryCodes me={me(1, 'ada', 'Ada', { totp_enabled: true, has_password: false })} returned={{ proof: 'oip_once', failed: false }} onClose={vi.fn()} onIssued={vi.fn()} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText('Code from your authenticator app'), '135790');
  await user.click(screen.getByRole('button', { name: 'Generate new codes' }));
  await waitFor(() => expect(calls.replace).toHaveBeenCalledWith({ identity_proof: 'oip_once', totp: '135790' }));
  expect(new URL(freshProofAddress('corp', 'recovery_codes', '/settings/security'), window.location.href).searchParams.get('sensitive_purpose')).toBe('recovery_codes');
});

it('the displayed codes can all be copied without a read-back request', async () => {
  const user = userEvent.setup();
  const clipboard = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
  const close = vi.fn();
  show(<SavedRecoveryCodes codes={CODES} onClose={close} />);
  await user.click(screen.getByRole('button', { name: 'Copy all recovery codes' }));
  expect(clipboard).toHaveBeenCalledWith(CODES.join('\n'));
  await user.click(screen.getByRole('button', { name: 'Close' }));
  expect(close).toHaveBeenCalledOnce();
  expect(calls.status).not.toHaveBeenCalled();
});

it('an authenticator disabled elsewhere offers setup instead of an impossible replacement', async () => {
  calls.status.mockResolvedValue(status({ remaining: 0, methods: [], created_at: null }));
  show(<ReplaceRecoveryCodes me={me(1, 'ada', 'Ada', { totp_enabled: true })} returned={null} onClose={vi.fn()} onIssued={vi.fn()} />);
  expect(await screen.findByText('Your authenticator is no longer enabled. Set it up again to receive recovery codes.')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Generate new codes' })).not.toBeInTheDocument();
  expect(calls.replace).not.toHaveBeenCalled();
});
