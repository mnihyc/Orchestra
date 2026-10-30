/** Mailed sign-in codes report failed sends and keep the reader's progress across retries. */
import type { AnchorHTMLAttributes } from 'react';
import type * as Router from '@tanstack/react-router';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Auth from '@/api/auth';
import { ApiError } from '@/api/client';
import { RESEND_AFTER_MS } from '@/features/auth/EmailCode';
import type { AuthCardProps } from '@/features/auth/Frame';
import { Login } from '@/features/auth/Login';
import type { Meta } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { NetworkError } from '@/lib/requests';
import { meta } from '../doubles/fixtures';
import { deferred } from './deferred';
import { show } from './support';

const calls = vi.hoisted(() => ({
  meta: vi.fn<() => Promise<Meta>>(),
  signIn: vi.fn<typeof Auth.signIn>(),
  askCode: vi.fn<typeof Auth.askEmailCode>(),
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(),
  metaQuery: { queryKey: ['code-delivery-meta'], queryFn: calls.meta },
  signIn: calls.signIn, askEmailCode: calls.askCode,
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useRouter: () => ({ navigate: vi.fn(), history: { replace: vi.fn() } }),
  Link: ({ to, children, className }: Pick<AnchorHTMLAttributes<HTMLAnchorElement>, 'children' | 'className'> & { to: string }) => <a href={to} className={className}>{children}</a>,
}));
vi.mock('@/features/auth/Frame', () => ({
  AuthCard: ({ children, foot }: AuthCardProps) => <section>{children}<footer>{foot}</footer></section>,
}));

const appearance = useAppearance.getState();
const secondStep = () => new ApiError({ type: 'orch:error:auth.second_step_required', code: 'auth.second_step_required', status: 403,
  title: 'Second step', detail: 'Another proof is required.', request_id: 'proof-request', methods: ['email_code'] }, null);
const networkFailure = () => new NetworkError('offline');

beforeEach(() => {
  calls.meta.mockReset().mockResolvedValue(meta({ mail: true, email_code_login: true }));
  calls.signIn.mockReset();
  calls.askCode.mockReset().mockResolvedValue(undefined);
});
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps the first sign-in form and offers retry when a code request fails in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme });
    calls.askCode.mockRejectedValueOnce(networkFailure());
    show(<Login onward={null} callbackError={null} />);
    const user = userEvent.setup();
    const name = await screen.findByLabelText(i18n.t('auth.fields.login'));
    await user.type(name, 'reader');
    await user.click(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') }));
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('network.offline'));
    expect(name).toHaveValue('reader');
    expect(screen.queryByLabelText(i18n.t('auth.code.label'))).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') }));
    expect(await screen.findByLabelText(i18n.t('auth.code.label'))).toBeVisible();
    expect(screen.getByText(i18n.t('auth.code.sent'))).toBeVisible();
    expect(calls.askCode.mock.calls.map(([body]) => body)).toEqual([{ login: 'reader' }, { login: 'reader' }]);
  });
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`does not claim a generic accepted response proves mail was sent in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    const user = userEvent.setup();
    const first = show(<Login onward={null} callbackError={null} />);
    await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
    await user.click(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') }));
    expect(await screen.findByText(locale === 'en'
      ? 'If that name may sign in this way and a code can be sent, it is on its way to its address.'
      : '如果这个账号可以用这种方式登录且验证码可以发送，验证码会发往它的邮箱。')).toBeVisible();
    first.unmount();
    calls.signIn.mockRejectedValueOnce(secondStep());
    show(<Login onward={null} callbackError={null} />);
    await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
    await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'first-proof');
    await user.click(screen.getByRole('button', { name: i18n.t('auth.login.submit') }));
    expect(await screen.findByText(locale === 'en'
      ? 'If a code can be sent, it is on its way to your address.'
      : '如果验证码可以发送，它会发往你的邮箱。')).toBeVisible();
  });
}

it('keeps a pending send from being submitted twice or replaced by password sign-in', async () => {
  await setLocale('en');
  const pending = deferred<void>();
  calls.askCode.mockReturnValueOnce(pending.promise);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.click(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') }));
  expect(screen.getByRole('button', { name: i18n.t('auth.code.sending') })).toBeDisabled();
  expect(screen.getByRole('button', { name: i18n.t('auth.login.submit') })).toBeDisabled();
  expect(screen.queryByLabelText(i18n.t('auth.code.label'))).not.toBeInTheDocument();
  expect(calls.askCode).toHaveBeenCalledTimes(1);
  await act(async () => { pending.reject(networkFailure()); await Promise.resolve(); });
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('network.offline'));
});

it('keeps the first proof and code field when the second-step send fails and is retried', async () => {
  await setLocale('en');
  calls.signIn.mockRejectedValueOnce(secondStep());
  calls.askCode.mockRejectedValueOnce(networkFailure());
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'first-proof');
  await user.click(screen.getByRole('button', { name: i18n.t('auth.login.submit') }));
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('network.offline'));
  expect(screen.queryByText(i18n.t('auth.second.codeSent'))).not.toBeInTheDocument();
  const field = screen.getByLabelText(i18n.t('auth.code.label'));
  await user.type(field, 'saved-code');
  await user.click(screen.getByRole('button', { name: i18n.t('auth.code.send') }));
  expect(await screen.findByText(i18n.t('auth.second.codeSent'))).toBeVisible();
  expect(field).toHaveValue('saved-code');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(calls.askCode.mock.calls.map(([body]) => body)).toEqual([
    { login: 'reader', password: 'first-proof' }, { login: 'reader', password: 'first-proof' },
  ]);
});

it('keeps the entered code and leaves a failed resend immediately retryable', async () => {
  await setLocale('en');
  let clock = Date.now();
  vi.spyOn(Date, 'now').mockImplementation(() => clock);
  calls.askCode.mockRejectedValueOnce(networkFailure());
  calls.askCode.mockResolvedValueOnce(undefined);
  calls.askCode.mockRejectedValueOnce(networkFailure());
  calls.askCode.mockResolvedValueOnce(undefined);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.click(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') }));
  await screen.findByRole('alert');
  await user.click(screen.getByRole('button', { name: i18n.t('auth.login.mailCode') }));
  const field = await screen.findByLabelText(i18n.t('auth.code.label'));
  await user.type(field, 'already-typed');
  clock += RESEND_AFTER_MS + 1_000;
  const resend = await screen.findByRole('button', { name: i18n.t('auth.code.resend') }, { timeout: 3_000 });
  await user.click(resend);
  expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('network.offline'));
  expect(field).toHaveValue('already-typed');
  expect(resend).toBeEnabled();
  await user.click(resend);
  await waitFor(() => expect(calls.askCode).toHaveBeenCalledTimes(4));
  expect(field).toHaveValue('already-typed');
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: /Send another in/u })).toBeDisabled();
});

it('does not return to a second step when a send completes after starting over', async () => {
  await setLocale('en');
  const pending = deferred<void>();
  calls.signIn.mockRejectedValueOnce(secondStep());
  calls.askCode.mockReturnValueOnce(pending.promise);
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText(i18n.t('auth.fields.login')), 'reader');
  await user.type(screen.getByLabelText(i18n.t('auth.fields.password'), { exact: true }), 'first-proof');
  await user.click(screen.getByRole('button', { name: i18n.t('auth.login.submit') }));
  expect(await screen.findByText(i18n.t('auth.code.sending'))).toBeVisible();
  await user.click(screen.getByRole('button', { name: i18n.t('auth.login.startOver') }));
  await act(async () => { pending.resolve(); await Promise.resolve(); });
  expect(screen.getByLabelText(i18n.t('auth.fields.login'))).toBeVisible();
  expect(screen.queryByText(i18n.t('auth.second.codeSent'))).not.toBeInTheDocument();
});
