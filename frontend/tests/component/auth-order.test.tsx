/** Password sign-in comes before provider choices in the page and the keyboard, including a return to an invitation. */
import type { AnchorHTMLAttributes } from 'react';
import type * as routerApi from '@tanstack/react-router';
import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';

import type * as authApi from '@/api/auth';
import type { AuthCardProps } from '@/features/auth/Frame';
import { Login } from '@/features/auth/Login';
import type { Meta } from '@/generated/api-types';
import { setLocale } from '@/i18n';

import { me, meta } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({
  readMeta: vi.fn<() => Promise<Meta>>(),
  signIn: vi.fn<typeof authApi.signIn>(),
  askEmailCode: vi.fn<typeof authApi.askEmailCode>(),
  resendConfirmation: vi.fn<typeof authApi.resendConfirmation>(),
  navigate: vi.fn<(...args: unknown[]) => Promise<void>>(),
  replace: vi.fn(),
}));

vi.mock('@/api/auth', async (original) => ({
  ...(await original<typeof authApi>()),
  metaQuery: { queryKey: ['auth-order-meta'], queryFn: calls.readMeta },
  signIn: calls.signIn,
  askEmailCode: calls.askEmailCode,
  resendConfirmation: calls.resendConfirmation,
}));
vi.mock('@tanstack/react-router', async (original) => ({
  ...(await original<typeof routerApi>()),
  useRouter: () => ({ navigate: calls.navigate, history: { replace: calls.replace } }),
  Link: ({ to, children, className }: Pick<AnchorHTMLAttributes<HTMLAnchorElement>, 'children' | 'className'> & { to: string }) => <a href={to} className={className}>{children}</a>,
}));
vi.mock('@/features/auth/Frame', () => ({
  AuthCard: ({ children, lead, foot }: AuthCardProps) => <section>{lead !== undefined && <p>{lead}</p>}{children}<footer>{foot}</footer></section>,
}));

const providers = [{ key: 'acme', name: 'Acme SSO', auto_register: false }, { key: 'beta', name: 'Beta SSO', auto_register: false }];

beforeEach(async () => {
  vi.clearAllMocks();
  await setLocale('en');
  calls.readMeta.mockResolvedValue(meta({ identity_providers: providers, email_code_login: true }));
  calls.signIn.mockResolvedValue({
    access_token: 'access-from-the-component-test',
    access_expires_at: '2026-10-15T12:00:00.000Z',
    user: me(1, 'ada', 'Ada'),
  });
  calls.navigate.mockResolvedValue(undefined);
  calls.askEmailCode.mockResolvedValue(undefined);
  calls.resendConfirmation.mockResolvedValue(undefined);
});

it.each([
  { visit: 'direct sign-in', onward: null },
  { visit: 'an invitation', onward: '/invite/still-valid' },
])('tabs through the password, its controls and Sign in before providers for $visit', async ({ onward }) => {
  show(<Login onward={onward} callbackError={null} />);
  const user = userEvent.setup();
  const username = await screen.findByLabelText('Username or email address');
  await waitFor(() => expect(username).toHaveFocus());
  const first = screen.getByRole('link', { name: 'Continue with Acme SSO' });
  const stops = [
    screen.getByLabelText('Password', { exact: true }),
    screen.getByRole('button', { name: 'Show password' }),
    screen.getByRole('link', { name: 'Forgot your password?' }),
    screen.getByRole('button', { name: 'Sign in' }),
    first,
    screen.getByRole('link', { name: 'Continue with Beta SSO' }),
  ];
  for (const stop of stops) {
    await user.tab();
    expect(stop).toHaveFocus();
  }
  await user.tab();
  expect(screen.getByRole('button', { name: 'Mail me a code instead' })).toHaveFocus();
  const href = first.getAttribute('href');
  expect(href).not.toBeNull();
  expect(new URL(href ?? '', window.location.href).searchParams.get('redirect')).toBe(onward);
});

it('places providers after the password form and its or divider', async () => {
  show(<Login onward={null} callbackError={null} />);
  const submit = await screen.findByRole('button', { name: 'Sign in' });
  const divider = screen.getByRole('separator');
  const provider = screen.getByRole('link', { name: 'Continue with Acme SSO' });
  expect(divider).toHaveTextContent('or');
  expect(submit.compareDocumentPosition(divider) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  expect(divider.compareDocumentPosition(provider) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
  expect(submit.closest('form')).not.toContainElement(provider);
});

it('shows providers alone without password fields or an or divider when password sign-in is off', async () => {
  calls.readMeta.mockResolvedValue(meta({ local_login: false, identity_providers: providers }));
  show(<Login onward={null} callbackError={null} />);
  expect(await screen.findByRole('link', { name: 'Continue with Acme SSO' })).toBeVisible();
  expect(screen.getByRole('link', { name: 'Continue with Beta SSO' })).toBeVisible();
  expect(screen.queryByLabelText('Username or email address')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('Password', { exact: true })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Sign in' })).not.toBeInTheDocument();
  expect(screen.queryByRole('separator')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Administrator sign-in' })).toBeVisible();
  await userEvent.tab();
  expect(screen.getByRole('link', { name: 'Continue with Acme SSO' })).toHaveFocus();
});

it('submits a password sign-in by Enter in the password field', async () => {
  show(<Login onward={null} callbackError={null} />);
  const user = userEvent.setup();
  await user.type(await screen.findByLabelText('Username or email address'), 'ada');
  await user.type(screen.getByLabelText('Password', { exact: true }), 'correct horse battery');
  await user.keyboard('{Enter}');
  await waitFor(() => expect(calls.signIn).toHaveBeenCalledExactlyOnceWith({ login: 'ada', password: 'correct horse battery' }));
  expect(calls.askEmailCode).not.toHaveBeenCalled();
  await waitFor(() => expect(calls.navigate).toHaveBeenCalledWith({ to: '/', replace: true }));
});
