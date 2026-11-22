/** An Inbox target reads one exact request, irrespective of the list's first page, and uses its returned decision. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Approvals from '@/api/approvals';
import type * as Auth from '@/api/auth';
import type * as Live from '@/realtime/live';
import type * as Screen from '@/shell/screen';
import { CardFrame } from '@/features/approvals/Card';
import { RequestedEnd } from '@/features/access/RequestEnd';
import { ApprovalDetail } from '@/features/approvals/ApprovalList';
import type { Approval } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { me, userRef } from '../doubles/fixtures';
import { ALLOW_DENY, harnessApproval } from '../doubles/thread-rows';
import { show } from './support';
const calls = vi.hoisted(() => ({ live: true, read: vi.fn<(_id: string) => Promise<Approval>>(), list: vi.fn(), decide: vi.fn<typeof Approvals.decideApproval>() }));
const ada = userRef(me(1, 'ada', 'Ada'));
const request = harnessApproval('beyond-first-page', { threadId: 'thread', itemId: 'item', title: 'Check the redirect', requester: ada,
  request: { request_kind: 'command', detail: { command: 'check redirect', cwd: '/workspace', reason: null }, options: ALLOW_DENY } });
vi.mock('@/api/approvals', async (original) => ({ ...await original<typeof Approvals>(),
  approvalQuery: (id: string) => ({ queryKey: ['exact-approval', id], queryFn: () => calls.read(id) }),
  approvalsQuery: (...args: Parameters<typeof Approvals.approvalsQuery>) => { calls.list(...args); throw new Error('An exact target does not open a list'); },
  decideApproval: (...args: Parameters<typeof Approvals.decideApproval>) => calls.decide(...args),
}));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof Auth>(), meQuery: { queryKey: ['exact-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
vi.mock('@/realtime/live', async (original) => ({ ...await original<typeof Live>(), useUserFeed: () => undefined, useSocketStatus: () => ({ phase: calls.live ? 'ready' : 'reconnecting' }) }));
vi.mock('@/shell/screen', async (original) => ({ ...await original<typeof Screen>(), useScreenHeading: () => undefined }));
vi.mock('@tanstack/react-router', async (original) => ({ ...await original<typeof Router>(), Link: ({ children }: { children: ReactNode }) => <a href="#thread">{children}</a> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <span>{user.display_name}</span> }));
const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const before = useAppearance.getState();
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');
beforeEach(() => { vi.clearAllMocks(); calls.live = true; calls.read.mockResolvedValue(request); calls.decide.mockResolvedValue({ ...request, state: 'approved', decision: { option_id: 'allow_once' }, decided_at: '2026-10-16T00:00:00Z', decided_by: ada }); });
afterEach(async () => {
  cleanup(); useAppearance.setState(before); document.documentElement.classList.toggle('dark', darkBefore);
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore;
  await setLocale('en');
});
it.each(modes)('opens the requested card alone and applies one decision without list repair in %s/%s', async (locale, theme) => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const view = show(<ApprovalDetail id={request.id} />);
  expect(await screen.findByText('check redirect')).toBeVisible();
  expect(view.container.querySelector('[data-focused-approval]')).toHaveAttribute('data-focused-approval', request.id);
  expect(calls.read).toHaveBeenCalledExactlyOnceWith(request.id);
  expect(calls.list).not.toHaveBeenCalled();
  const allowName = `${i18n.t('approvals.allowOnce')} · check redirect`;
  await userEvent.click(screen.getByRole('button', { name: allowName }));
  await waitFor(() => expect(calls.decide).toHaveBeenCalledExactlyOnceWith(request.id, { option_id: 'allow_once' }));
  await waitFor(() => {
    expect(screen.queryByRole('button', { name: allowName })).not.toBeInTheDocument();
    expect(screen.getByRole('region', { name: request.title })).toHaveAttribute('data-state', 'approved');
  });
  const settled = screen.getByRole('region', { name: request.title });
  const outcome = within(settled).getByRole('button', { expanded: false });
  expect(outcome).toHaveTextContent(i18n.t('approvals.allowedOnce'));
  expect(within(settled).getByText('Ada')).toBeVisible();
  expect(outcome).not.toContainElement(within(settled).getByText('Ada'));
  expect(calls.read).toHaveBeenCalledTimes(1);
  expect(calls.list).not.toHaveBeenCalled();
});

for (const locale of ['en', 'zh-CN'] as const) it.each(['authority', 'connection'] as const)('explains the unavailable approval '+locale+' %s', async (cause) => {
  await setLocale(locale);
  calls.live = cause !== 'connection'; calls.read.mockResolvedValue({ ...request, can_decide: cause !== 'authority' });
  show(<ApprovalDetail id={request.id} />);
  const control = await screen.findByRole('button', { name: `${i18n.t('approvals.allowOnce')} · check redirect` }), target = control.parentElement;
  if (target === null) throw new Error('The disabled approval must have a focusable reason');
  expect(control).toBeDisabled(); act(() => target.focus());
  expect(await screen.findByRole('tooltip')).toHaveTextContent(i18n.t(cause === 'authority' ? 'approvals.cannotAnswer' : 'approvals.waitForLine'));
  expect(calls.decide).not.toHaveBeenCalled();
});

it.each(['en','zh-CN'] as const)('keeps the decision deadline distinct from a requested access end in %s',async(locale)=>{
  await setLocale(locale);
  const decisionEnd='2030-11-15T12:00:00.000Z',grantEnd='2030-11-01T12:05:00.000Z';
  const access:Approval={...request,kind:'access',expires_at:decisionEnd,request:{permissions:['thread.view'],options:null,holder:{kind:'user',user:ada},reason:'Read the thread',for_thread:null,expires_at:grantEnd}};
  const view=show(<CardFrame approval={access}><RequestedEnd at={grantEnd}/></CardFrame>);
  const label=screen.getByText(locale==='en'?'Decision until':'审批有效至');
  expect(label.parentElement?.querySelector('time')).toHaveAttribute('dateTime',decisionEnd);
  expect(view.container.querySelector(`time[datetime="${grantEnd}"]`)).not.toBeNull();
});

it.each(['en','zh-CN'] as const)('says an undated program request lasts while that program waits in %s',async(locale)=>{
  await setLocale(locale);show(<CardFrame approval={request}><span>Command</span></CardFrame>);
  expect(screen.getByText(locale==='en'?'While the agent waits':'代理程序等待期间有效')).toBeVisible();
  expect(screen.queryByText(locale==='en'?'Decision until':'审批有效至')).not.toBeInTheDocument();
});
