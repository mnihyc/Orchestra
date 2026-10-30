/** Access request outcomes, withdrawal and remembered state follow the current reader and server. */
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as Access from '@/api/access';
import type * as Approvals from '@/api/approvals';
import { ApiError, client } from '@/api/client';
import { AccessRequest } from '@/features/access/AccessRequest';
import type { AccessRequestGetReply, Approval, MissingAccess, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { NetworkError } from '@/lib/requests';
import { ADA, BEA, TOPIC } from '../unit/general-access-fixtures';
import { deferred } from './deferred';
import { show } from './support';

const approval: Extract<Approval, { kind: 'access' }> = {
  id: 'first', batch_id: 'batch', kind: 'access', state: 'pending', title: 'Review Release',
  object: { kind: 'thread', id: 'release', name: 'Release details' }, node: TOPIC, requester: { user: BEA },
  request: { permissions: ['thread.view', 'thread.interact'], reason: 'Review the work', holder: { kind: 'user', user: BEA }, options: null, for_thread: null },
  required_permission: 'access.manage', can_decide: true, approver_scope: null, created_at: '2026-10-15T21:31:00Z',
  decided_at: null, decided_automatically: false, decided_by: null, decision: null, expires_at: null, item_id: null, thread_id: null,
};
const missing: MissingAccess = { object: { kind: 'thread', id: 'release', name: 'Release', owner: ADA },
  permissions: ['thread.view', 'thread.interact'], options: null, ask: [ADA], ask_more: 0, override: false, closed: null };
const denied = { ...approval, state: 'denied' as const, decided_at: '2026-10-26T12:00:00Z', decided_by: ADA,
  decision: { approve: false, note: 'Please use the shared copy' } };
const partial = { ...approval, state: 'approved' as const, decided_at: '2026-10-26T12:00:00Z', decided_by: ADA,
  decision: { approve: true, permissions: ['thread.view'] } };
const calls = vi.hoisted(() => ({
  request: vi.fn<typeof Access.requestAccess>(),
  read: vi.fn<(batch: string) => Promise<AccessRequestGetReply>>(),
  cancel: vi.fn<typeof Access.cancelApproval>(),
  approval: vi.fn<(id: string) => Promise<Approval>>(),
}));
vi.mock('@/api/access', async (load) => ({ ...await load<typeof Access>(),
  requestAccess: (...args: Parameters<typeof Access.requestAccess>) => calls.request(...args),
  cancelApproval: (...args: Parameters<typeof Access.cancelApproval>) => calls.cancel(...args),
  accessRequestQuery: (batch: string) => ({ queryKey: ['recovery-batch', batch], queryFn: () => calls.read(batch), retry: false }),
}));
vi.mock('@/api/approvals', async (load) => ({ ...await load<typeof Approvals>(),
  approvalQuery: (id: string) => ({ queryKey: ['recovery-approval', id], queryFn: () => calls.approval(id), retry: false }),
}));
vi.mock('@/realtime/stores/tree', () => ({ useNodePath: () => ({ path: [] }) }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: UserRef }) => <span>{user.display_name}</span> }));
const originalSession = client.session.getState();

beforeEach(async () => {
  await setLocale('en');
  sessionStorage.clear();
  client.session.setState({ status: 'signed_in', userId: 'reader-a', readerVersion: 111 });
  calls.request.mockReset().mockResolvedValue({ batch_id: 'batch', approvals: [approval] });
  calls.read.mockReset().mockResolvedValue({ batch_id: 'batch', approvals: [approval], complete: false });
  calls.cancel.mockReset().mockResolvedValue({ ...approval, state: 'cancelled' });
  calls.approval.mockReset().mockResolvedValue(approval);
});
afterEach(async () => { cleanup(); client.session.setState(originalSession); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const outcome of ['denied', 'partial'] as const) {
  it(`shows an answered ${outcome} request and allows another request in ${locale}`, async () => {
    await setLocale(locale);
    calls.read.mockResolvedValue({ batch_id: 'batch', approvals: [outcome === 'denied' ? denied : partial], complete: false });
    show(<AccessRequest missing={[missing]} objectId="release" fixed />);
    const user = userEvent.setup();
    await user.type(screen.getByRole('textbox', { name: i18n.t('access.request.reason') }), 'Please review');
    await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
    expect(await screen.findByText(i18n.t('access.request.answered'))).toBeVisible();
    expect(screen.queryByText(i18n.t('access.request.waitingFor', { names: ADA.display_name }))).not.toBeInTheDocument();
    const again = screen.getByRole('button', { name: i18n.t('access.request.askAgain') });
    expect(again).toBeEnabled();
    await user.click(again);
    expect(screen.getByRole('textbox', { name: i18n.t('access.request.reason') })).toHaveValue('Please review');
    await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
    await waitFor(() => expect(calls.request).toHaveBeenCalledTimes(2));
    expect(calls.request.mock.calls[0]?.[1]).not.toBe(calls.request.mock.calls[1]?.[1]);
  });
}

it('withdraws only the requests that remain pending after another item was answered', async () => {
  const pending = { ...approval, id: 'second' };
  calls.request.mockResolvedValue({ batch_id: 'batch', approvals: [approval, pending] });
  calls.read.mockResolvedValue({ batch_id: 'batch', approvals: [partial, pending], complete: false });
  show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await screen.findByText(/given in part/u);
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.cancel') }));
  await waitFor(() => expect(calls.cancel).toHaveBeenCalledTimes(1));
  expect(calls.cancel).toHaveBeenCalledWith('second');
  expect(await screen.findByRole('button', { name: i18n.t('access.request.send') })).toBeEnabled();
});

it('shows a failed batch refresh and retries it without discarding the request', async () => {
  calls.read.mockRejectedValueOnce(new NetworkError('offline'));
  show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('network.offline'));
  await user.click(within(alert).getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  expect(await screen.findByRole('button', { name: i18n.t('access.request.cancel') })).toBeEnabled();
});

it('shows and retries a failed read of an already pending approval', async () => {
  calls.approval.mockRejectedValueOnce(new NetworkError('offline'));
  show(<AccessRequest missing={[missing]} objectId="release" fixed pendingId="first" />);
  const alert = await screen.findByRole('alert');
  expect(alert).toHaveTextContent(i18n.t('network.offline'));
  await userEvent.click(within(alert).getByRole('button', { name: i18n.t('common.tryAgain') }));
  await waitFor(() => expect(calls.approval).toHaveBeenCalledTimes(2));
  expect(await screen.findByRole('button', { name: i18n.t('access.request.cancel') })).toBeEnabled();
});

it('does not show a stored request or its details to a different reader in the same tab', async () => {
  const view = show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  expect(await screen.findByText(/Release details/u)).toBeVisible();
  const stored = Object.values(sessionStorage).join(' ');
  expect(stored).toContain('batch');
  expect(stored).not.toContain('Release details');
  act(() => client.session.setState({ status: 'signed_in', userId: 'reader-b', readerVersion: 112 }));
  expect(screen.getByRole('button', { name: i18n.t('access.request.send') })).toBeEnabled();
  expect(view.container).not.toHaveTextContent('Release details');
  expect(calls.read).toHaveBeenCalledTimes(1);
});

it('does not store a request raised from a private reference', async () => {
  show(<AccessRequest missing={[missing]} objectId="release" fixed privateReference />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  expect(await screen.findByRole('button', { name: i18n.t('access.request.cancel') })).toBeEnabled();
  expect(sessionStorage).toHaveLength(0);
});

it('loads current request details after a reload instead of displaying remembered details', async () => {
  const view = show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  expect(await screen.findByText(/Release details/u)).toBeVisible();
  view.unmount();
  const pending = deferred<AccessRequestGetReply>();
  calls.read.mockReturnValueOnce(pending.promise);
  const reopened = show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  await waitFor(() => expect(calls.read).toHaveBeenCalledTimes(2));
  expect(reopened.container).not.toHaveTextContent('Release details');
  await act(async () => { pending.resolve({ batch_id: 'batch', approvals: [denied], complete: false }); await Promise.resolve(); });
  expect(await screen.findByText(i18n.t('access.request.answered'))).toBeVisible();
});

it('does not reuse the previous request when the target changes', async () => {
  const view = show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  expect(await screen.findByText(/Release details/u)).toBeVisible();
  const other = { ...missing, object: { ...missing.object, id: 'another', name: 'Another thread' } };
  view.rerender(<AccessRequest missing={[other]} objectId="another" fixed />);
  expect(screen.getByRole('button', { name: i18n.t('access.request.send') })).toBeEnabled();
  expect(view.container).not.toHaveTextContent('Release details');
});

it('hides prior request details when the batch is no longer readable', async () => {
  calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404,
    title: 'Not found', detail: 'Not found', request_id: 'gone' }, null));
  const view = show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  const alert = await screen.findByRole('alert');
  expect(alert).toBeVisible();
  expect(view.container).not.toHaveTextContent('Release details');
  await userEvent.click(within(alert).getByRole('button', { name: i18n.t('access.request.askAgain') }));
  expect(screen.getByRole('button', { name: i18n.t('access.request.send') })).toBeEnabled();
});

it('shows a private request end only while its approval is readable', async () => {
  const timed = { ...approval, request: { ...approval.request, expires_at: '2100-04-05T12:34:56.123Z' } };
  calls.approval.mockResolvedValueOnce(timed);
  const view = show(<AccessRequest missing={[missing]} objectId="release" fixed privateReference pendingId="first" />);
  expect(await screen.findByRole('button', { name: i18n.t('access.request.cancel') })).toBeEnabled();
  expect(view.container.querySelector('[data-requested-end] time')).toHaveAttribute('datetime', timed.request.expires_at);
  view.unmount();
  calls.request.mockResolvedValueOnce({ batch_id: 'batch', approvals: [timed] });
  calls.read.mockRejectedValueOnce(new ApiError({ type: 'orch:error:not_found', code: 'not_found', status: 404,
    title: 'Not found', detail: 'Not found', request_id: 'gone' }, null));
  const hidden = show(<AccessRequest missing={[missing]} objectId="release" fixed privateReference />);
  await userEvent.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(hidden.container.querySelector('[data-requested-end]')).toBeNull();
  await userEvent.click(within(screen.getByRole('alert')).getByRole('button', { name: i18n.t('access.request.askAgain') }));
  expect(screen.getByRole('button', { name: i18n.t('access.request.send') })).toBeEnabled();
});

it('starts a fresh requested duration when an answered request is sent again', async () => {
  const now = Date.now();
  const clock = vi.spyOn(Date, 'now').mockReturnValue(now);
  calls.read.mockResolvedValue({ batch_id: 'batch', approvals: [denied], complete: false });
  show(<AccessRequest missing={[missing]} objectId="release" fixed />);
  const user = userEvent.setup();
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.request.duration') }));
  await user.click(screen.getByRole('option', { name: '1 week' }));
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await screen.findByText(i18n.t('access.request.answered'));
  const first = calls.request.mock.calls[0]?.[0].expires_at;
  expect(first).toBeDefined();
  clock.mockReturnValue(now + 60_000);
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.askAgain') }));
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await waitFor(() => expect(calls.request).toHaveBeenCalledTimes(2));
  expect(Date.parse(calls.request.mock.calls[1]?.[0].expires_at ?? '') - Date.parse(first ?? '')).toBe(60_000);
  clock.mockRestore();
});
