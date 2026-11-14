/** One confirmed bulk session act updates Security from its answer, with this browser retained. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Auth from '@/api/auth';
import type * as Users from '@/api/users';
import { beforeEach, expect, it, vi } from 'vitest';

import { answerChanges } from '@/api/actAnswers';
import { applyChanges } from '@/api/acts';
import { Security } from '@/features/settings/personal/Security';
import type { SessionInfo } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { me, meta } from '../doubles/fixtures';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const calls = vi.hoisted(() => ({ list: vi.fn<() => Promise<SessionInfo[]>>(), searches: vi.fn<(q: string) => void>(), revoke: vi.fn<typeof Users.revokeOtherSessions>() }));
vi.mock('@/api/auth', async (original) => ({ ...(await original<typeof Auth>()),
  meQuery: { queryKey: ['me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) },
  metaQuery: { queryKey: ['meta'], queryFn: () => Promise.resolve(meta()) },
}));
vi.mock('@/api/users', async (original) => ({ ...(await original<typeof Users>()),
  sessionsQuery: { queryKey: ['sessions'], queryFn: calls.list }, revokeOtherSessions: calls.revoke,
  sessionsPageQuery: (q: string) => ({ queryKey: ['sessions', 'pages', q], initialPageParam: null,
    queryFn: async () => { calls.searches(q); const rows = await calls.list(); return { items: rows.filter((row) => (row.user_agent + ' ' + (row.ip ?? '')).includes(q)).slice(0, 50), next_cursor: null }; }, getNextPageParam: () => null }),
}));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));

const current: SessionInfo = { id: 'current', current: true, user_agent: 'Chrome/130 Linux', ip: '203.0.113.1', created_at: '2026-10-15T12:00:00.000Z', last_used_at: '2026-10-15T12:00:00.000Z', expires_at: '2026-11-15T12:00:00.000Z' };
const other: SessionInfo = { ...current, id: 'other', current: false, user_agent: 'Firefox/130 Windows' };

beforeEach(async () => { vi.clearAllMocks(); await setLocale('en'); calls.list.mockResolvedValue([current, other]); });

it.each(['en', 'zh-CN'] as const)('confirms once, retains this browser and consumes the exact answer in %s', async (locale) => {
  await setLocale(locale);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  calls.revoke.mockImplementation(async () => { await applyChanges(answerChanges({ method: 'DELETE', path: '/me/sessions', answer: [current] }, () => '', [{ key: ['sessions'] }]), client); return [current]; });
  show(<QueryClientProvider client={client}><Security /></QueryClientProvider>);
  const user = userEvent.setup();
  const button = await screen.findByRole('button', { name: locale === 'en' ? 'Sign out other sessions' : '退出其他会话' });
  await user.click(button);
  let confirm = screen.getByRole('alertdialog');
  await user.click(within(confirm).getByRole('button', { name: locale === 'en' ? 'Cancel' : '取消' }));
  expect(calls.revoke).not.toHaveBeenCalled();
  await user.click(button);
  confirm = screen.getByRole('alertdialog');
  await user.click(within(confirm).getByRole('button', { name: locale === 'en' ? 'Sign out other sessions' : '退出其他会话' }));
  await waitFor(() => expect(calls.revoke).toHaveBeenCalledOnce());
  await waitFor(() => expect(screen.queryByRole('alertdialog')).not.toBeInTheDocument());
  expect(screen.getByText('Chrome (Linux)')).toBeVisible();
  await waitFor(() => expect(screen.queryByText('Firefox (Windows)')).not.toBeInTheDocument());
  expect(client.getQueryData(['sessions'])).toEqual([current]);
  expect(calls.list).toHaveBeenCalledOnce();
  expect(answerChanges({ method: 'DELETE', path: '/me/sessions', answer: [current] }, () => '', [{ key: ['sessions'] }]))
    .toEqual([{ value: { key: ['sessions'], data: [current] } }]);
});

it('searches a browser past a thousand on the server and owns the resulting scroll', async () => {
  calls.list.mockResolvedValue(Array.from({ length: 1_001 }, (_, index) => ({ ...other, id: String(index), user_agent: 'Browser ' + String(index).padStart(4, '0') })));
  show(<Security />);
  const search = await screen.findByRole('searchbox', { name: 'Find in Sessions' });
  const user = userEvent.setup(); await user.type(search, 'Browser 1000');
  await waitFor(() => expect(calls.searches).toHaveBeenCalledWith('Browser 1000'));
  expect(await screen.findByText('Browser 1000')).toBeVisible();
  expect(screen.queryByText('Browser 0000')).toBeNull();
  expect(screen.getByRole('region', { name: 'Sessions' })).toHaveClass('max-h-96');
});

it('offers no bulk act when this is the only browser', async () => {
  calls.list.mockResolvedValue([current]);
  show(<Security />);
  expect(await screen.findByText('Chrome (Linux)')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Sign out other sessions' })).not.toBeInTheDocument();
});

it('retains the confirmation and session list after a refused act so it can be tried again', async () => {
  calls.revoke.mockRejectedValueOnce(new Error('offline')).mockResolvedValueOnce([current]);
  show(<Security />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Sign out other sessions' }));
  const confirm = screen.getByRole('alertdialog');
  await user.click(within(confirm).getByRole('button', { name: 'Sign out other sessions' }));
  await waitFor(() => expect(calls.revoke).toHaveBeenCalledOnce());
  expect(confirm).toBeVisible();
  expect(screen.getByText('Firefox (Windows)')).toBeInTheDocument();
  await user.click(within(confirm).getByRole('button', { name: 'Sign out other sessions' }));
  await waitFor(() => expect(calls.revoke).toHaveBeenCalledTimes(2));
});
