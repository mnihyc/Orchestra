/** One confirmed bulk session act updates Security from its answer, with this browser retained. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Auth from '@/api/auth';
import type * as Users from '@/api/users';
import { beforeEach, expect, it, vi } from 'vitest';

import { answerChanges } from '@/api/actAnswers';
import { client as browser } from '@/api/client';
import { NetworkError } from '@/lib/requests';
import { Security } from '@/features/settings/personal/Security';
import type { SessionInfo } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { me, meta } from '../doubles/fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ list: vi.fn<() => Promise<SessionInfo[]>>(), revoke: vi.fn<typeof Users.revokeOtherSessions>() }));
vi.mock('@/api/auth', async (original) => ({ ...(await original<typeof Auth>()),
  meQuery: { queryKey: ['me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) },
  metaQuery: { queryKey: ['meta'], queryFn: () => Promise.resolve(meta()) },
}));
vi.mock('@/api/users', async (original) => ({ ...(await original<typeof Users>()),
  sessionsQuery: { queryKey: ['sessions'], queryFn: calls.list }, revokeOtherSessions: calls.revoke,
}));
vi.mock('@/features/settings/PersonalFrame', () => ({ PersonalFrame: ({ children }: { children: ReactNode }) => <main>{children}</main> }));

const current: SessionInfo = { id: 'current', current: true, user_agent: 'Chrome/130 Linux', ip: '203.0.113.1', created_at: '2026-10-15T12:00:00.000Z', last_used_at: '2026-10-15T12:00:00.000Z', expires_at: '2026-11-15T12:00:00.000Z' };
const other: SessionInfo = { ...current, id: 'other', current: false, user_agent: 'Firefox/130 Windows' };

beforeEach(async () => { vi.clearAllMocks(); await setLocale('en'); calls.list.mockResolvedValue([current, other]); });

it('keeps the actual failed renewal on this browser alone without reading again and closes it for a replacement reader', async () => {
  const before = browser.session.getState(), person = me(1, 'ada', 'Ada');
  const fetch = vi.spyOn(globalThis, 'fetch').mockRejectedValue(new TypeError('Synthetic connection refusal'));
  const grant = { user: person, access_token: 'synthetic-held', access_expires_at: new Date(Date.now() + 900_000).toISOString() };
  browser.adopt(grant); const next = browser.session.getState().renewal?.scheduled_at;
  if (typeof next !== 'number') throw new Error('The held grant schedules its actual renewal');
  await expect(browser.renew()).rejects.toBeInstanceOf(NetworkError);
  const view = show(<Security />);
  try {
    const own = (await screen.findByText('Chrome (Linux)')).closest('li'), foreign = screen.getByText('Firefox (Windows)').closest('li');
    if (own === null || foreign === null) throw new Error('Each native session keeps its own existing row');
    expect(own).toHaveTextContent(i18n.t('clientFacts.renewalFailed'));
    expect(within(foreign).queryByRole('button', { name: i18n.t('common.details') })).not.toBeInTheDocument();
    await userEvent.click(within(own).getByRole('button', { name: i18n.t('common.details') }));
    expect(document.querySelector(`[data-fact-disclosure-body] time[datetime="${new Date(next).toISOString()}"]`)).toBeInTheDocument();
    expect(fetch).toHaveBeenCalledOnce(); expect(calls.list).toHaveBeenCalledOnce(); expect(document.body).not.toHaveTextContent('synthetic-held');
    act(() => browser.adopt({ ...grant, user: me(2, 'replacement', 'Replacement'), access_token: 'synthetic-replacement', access_expires_at: new Date(Date.now() + 1_800_000).toISOString() }));
    expect(screen.queryByText(i18n.t('clientFacts.renewalFailed'))).not.toBeInTheDocument();
    expect(document.querySelector('[data-fact-disclosure-body]')).toBeNull();
  } finally { view.unmount(); fetch.mockResolvedValue(new Response(null, { status: 204 })); await browser.signOut(); fetch.mockRestore(); browser.session.setState(before); }
});

it.each(['en', 'zh-CN'] as const)('confirms once, retains this browser and consumes the exact answer in %s', async (locale) => {
  await setLocale(locale);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  calls.revoke.mockImplementation(() => { client.setQueryData(['sessions'], [current]); return Promise.resolve([current]); });
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

it.each(['en','zh-CN'] as const)('names the sessions actual idle expiry in its existing source line in %s',async(locale)=>{
  await setLocale(locale);show(<Security/>);
  const browser=await screen.findByText('Chrome (Linux)'),row=browser.closest('li');
  if(row===null)throw new Error('The session has its own row');
  expect(row).toHaveTextContent(locale==='en'?'Idle expiry':'空闲到期');
  expect(calls.list).toHaveBeenCalledTimes(1);
});
