/** A failed read of another person's approval decision stays visible and can be retried. */
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { useQueryClient, type QueryClient } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Approvals from '@/api/approvals';
import type * as Auth from '@/api/auth';
import { ApiError } from '@/api/client';
import { WaitingList } from '@/features/approvals/ApprovalList';
import type { Approval } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import type * as Live from '@/realtime/live';
import { me, userRef } from '../doubles/fixtures';
import { ALLOW_DENY, harnessApproval } from '../doubles/thread-rows';
import { deferred } from './deferred';
import { show } from './support';

const ada = userRef(me(1, 'ada', 'Ada'));
const request = harnessApproval('another-decision', { threadId: 'thread', itemId: 'item', title: 'A private request', requester: ada,
  request: { request_kind: 'command', detail: { command: 'check redirect', cwd: '/workspace', reason: null }, options: ALLOW_DENY } });
const decided: Approval = { ...request, state: 'approved', decision: { option_id: 'allow_once' }, decided_at: '2026-10-30T12:00:00Z', decided_by: ada };
const calls = vi.hoisted(() => ({ read: vi.fn<(_id: string) => Promise<Approval>>(), list: vi.fn() }));
vi.mock('@/api/approvals', async (load) => ({ ...await load<typeof Approvals>(),
  approvalQuery: (id: string) => ({ queryKey: ['outcome-approval', id], queryFn: () => calls.read(id) }),
  approvalsQuery: () => ({ queryKey: ['outcome-list'], queryFn: calls.list, initialPageParam: null, getNextPageParam: () => undefined }),
}));
vi.mock('@/api/auth', async (load) => ({ ...await load<typeof Auth>(), meQuery: { queryKey: ['outcome-me'], queryFn: () => Promise.resolve(me(1, 'ada', 'Ada')) } }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useUserFeed: () => undefined, useSocketStatus: () => ({ phase: 'ready' }) }));
vi.mock('@/realtime/stores/tree', () => ({ useNodePath: () => ({ path: [] }) }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), Link: ({ children }: { children: ReactNode }) => <a href="#thread">{children}</a> }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: { display_name: string } }) => <span>{user.display_name}</span> }));

let cache: QueryClient;
function Capture() { cache = useQueryClient(); return null; }
const previous = useAppearance.getState();
const originalTheme = document.documentElement.dataset.theme;
beforeEach(() => { calls.read.mockReset(); calls.list.mockReset().mockResolvedValue({ items: [request], next_cursor: null }); });
afterEach(async () => {
  cleanup(); useAppearance.setState(previous); await setLocale('en');
  if (originalTheme === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = originalTheme;
});
function removeFromList() {
  act(() => cache.setQueryData(['outcome-list'], { pages: [{ items: [], next_cursor: null }], pageParams: [null] }));
}
function showWaiting() {
  return show(<><Capture /><WaitingList shape={{ filter: { scope: 'mine', state: 'pending' }, empty: { title: 'Nothing waits', description: '' } }} /></>);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps an outcome read in place and retries its failure in ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale, theme, resolvedTheme: theme }); document.documentElement.dataset.theme = theme;
    const pending = deferred<Approval>();
    calls.read.mockReturnValueOnce(pending.promise).mockResolvedValueOnce(decided);
    showWaiting();
    expect(await screen.findByText('check redirect')).toBeVisible();
    removeFromList();
    await waitFor(() => expect(calls.read).toHaveBeenCalledExactlyOnceWith(request.id));
    expect(screen.queryByText('Nothing waits')).not.toBeInTheDocument();
    await act(async () => {
      pending.reject(new ApiError({ type: 'orch:error:internal', code: 'internal', status: 503,
        title: 'Unavailable', detail: 'Try again', request_id: 'read-failed' }, null));
      await Promise.resolve();
    });
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent(i18n.t('approvals.outcomeUnavailable'));
    expect(screen.queryByText('A private request')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    expect(await screen.findByRole('region', { name: request.title })).toHaveAttribute('data-state', 'approved');
    expect(calls.read).toHaveBeenCalledTimes(2);
  });
}

it('does not show a failed outcome beside a request that returned to the waiting list', async () => {
  calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404,
    title: 'Not found', detail: 'Not found', request_id: 'read-gone' }, null));
  showWaiting();
  expect(await screen.findByText('check redirect')).toBeVisible();
  removeFromList();
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.queryByText('A private request')).not.toBeInTheDocument();
  act(() => cache.setQueryData(['outcome-list'], { pages: [{ items: [request], next_cursor: null }], pageParams: [null] }));
  expect(await screen.findByText('check redirect')).toBeVisible();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
});

it('keeps an unreadable newer outcome before an older waiting request', async () => {
  const earlier = { ...request, id: 'earlier', title: 'Earlier request', created_at: '2026-09-01T10:00:00Z' };
  calls.list.mockResolvedValue({ items: [request, earlier], next_cursor: null });
  calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:internal', code: 'internal', status: 503,
    title: 'Unavailable', detail: 'Try again', request_id: 'read-failed' }, null));
  const view = showWaiting();
  await waitFor(() => expect(screen.getAllByText('check redirect')).toHaveLength(2));
  act(() => cache.setQueryData(['outcome-list'], { pages: [{ items: [earlier], next_cursor: null }], pageParams: [null] }));
  const failed = (await screen.findByRole('alert')).closest('li');
  const old = view.container.querySelector('[data-approval="earlier"]')?.closest('li');
  if (failed === null || old === null || old === undefined) throw new Error('Both request rows must be shown.');
  expect(failed.compareDocumentPosition(old) & Node.DOCUMENT_POSITION_FOLLOWING).not.toBe(0);
});
