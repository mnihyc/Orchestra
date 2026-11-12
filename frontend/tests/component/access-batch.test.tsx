/** Reviewed access batches preserve each request's terms, partial outcomes and the acting reader. */
import { useQueryClient, type QueryClient } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Approvals from '@/api/approvals';
import { approvalKey } from '@/api/approvals';
import { ApiError, client } from '@/api/client';
import { AccessBatch } from '@/features/approvals/AccessBatch';
import type { Approval, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ADA, BEA, TOPIC } from '../unit/general-access-fixtures';
import { deferred } from './deferred';
import { show } from './support';

type AccessApproval = Extract<Approval, { kind: 'access' }>;
const first: AccessApproval = {
  id: 'first', batch_id: 'batch', kind: 'access', state: 'pending', title: 'Read the plan',
  object: { kind: 'thread', id: 'plan', name: 'Plan' }, node: TOPIC, requester: { user: BEA },
  request: { permissions: ['thread.view'], reason: 'Review the plan', holder: { kind: 'user', user: BEA },
    options: null, for_thread: null, expires_at: '2099-01-01T12:00:00Z' },
  required_permission: 'access.manage', can_decide: true, approver_scope: null, created_at: '2026-10-15T21:31:00Z',
  decided_at: null, decided_automatically: false, decided_by: null, decision: null, expires_at: null, item_id: null, thread_id: null,
};
const second: AccessApproval = { ...first, id: 'second', title: 'Use the model', request: { ...first.request, permissions: ['provider.use'], options: { models: ['sample-model'], efforts: ['low'] }, expires_at: null } };
const other: AccessApproval = { ...first, id: 'other', title: 'Another approver decides this', can_decide: false };
const decided = (request: AccessApproval, approve = true): AccessApproval => ({ ...request, state: approve ? 'approved' : 'denied', can_decide: false,
  decision: { approve }, decided_by: ADA, decided_at: '2026-10-15T22:00:00Z' });
const calls = vi.hoisted(() => ({ decide: vi.fn<typeof Approvals.decideApproval>(), settled: vi.fn<(_approval: Approval) => void>() }));
vi.mock('@/api/approvals', async (load) => ({ ...await load<typeof Approvals>(), decideApproval: calls.decide }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: UserRef }) => <button>{user.display_name}</button> }));

let cache: QueryClient;
function Capture() { cache = useQueryClient(); return null; }
const before = client.session.getState();
const summary = (request: AccessApproval) => <span>{request.request.permissions.join(', ')}</span>;
function Scene({ live = true, items = [first, second, other] }: { live?: boolean; items?: readonly AccessApproval[] }) {
  return <><Capture /><AccessBatch items={items} live={live} onSettled={calls.settled} summary={summary} /></>;
}
beforeEach(() => { vi.clearAllMocks(); client.session.setState({ ...before, userId: 'reader', readerVersion: 1, status: 'signed_in' });
  calls.decide.mockImplementation((id, decision) => Promise.resolve(decided(id === first.id ? first : second, 'approve' in decision ? decision.approve : true))); });
afterEach(async () => { cleanup(); client.session.setState(before); await setLocale('en'); });

async function open() {
  await userEvent.click(screen.getByRole('button', { name: i18n.t('approvals.batch.open') }));
  return screen.getByRole('dialog', { name: i18n.t('approvals.batch.title') });
}

for (const locale of ['en', 'zh-CN'] as const) {
  it(`reviews selected requests before sending exact permissions, options and expiry in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<Approval>(); calls.decide.mockReturnValueOnce(pending.promise);
    show(<Scene />);
    const dialog = await open();
    expect(calls.decide).not.toHaveBeenCalled();
    expect(within(dialog).getByRole('checkbox', { name: i18n.t('approvals.batch.selectNamed', { name: other.title }) })).toBeDisabled();
    const pickSecond = within(dialog).getByRole('checkbox', { name: i18n.t('approvals.batch.selectNamed', { name: second.title }) });
    await userEvent.click(pickSecond);
    const approve = () => within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') });
    await userEvent.click(approve());
    expect(approve()).toBeDisabled();
    expect(approve()).toHaveAttribute('aria-busy', 'true');
    await userEvent.click(approve());
    expect(calls.decide).toHaveBeenCalledExactlyOnceWith(first.id, { approve: true, permissions: ['thread.view'], expires_at: first.request.expires_at });
    await act(() => { pending.resolve(decided(first)); return Promise.resolve(); });
    await waitFor(() => expect(calls.settled).toHaveBeenCalledTimes(1));
    expect(approve()).toBeDisabled();
    await userEvent.click(pickSecond);
    await userEvent.click(approve());
    await waitFor(() => expect(calls.decide).toHaveBeenLastCalledWith(second.id, { approve: true, permissions: ['provider.use'], options: second.request.options }));
    await waitFor(() => expect(calls.settled).toHaveBeenCalledTimes(2));
    expect(calls.decide).toHaveBeenCalledTimes(2);
    expect(calls.settled.mock.calls.map(([row]) => row.id)).toEqual([first.id, second.id]);
  });

  it(`retains per-request failures and retries only selected undecided rows in ${locale}`, async () => {
    await setLocale(locale);
    calls.decide.mockRejectedValueOnce(new Error('The first decision was not confirmed')).mockResolvedValueOnce(decided(second));
    show(<Scene items={[first, second]} />);
    const dialog = await open();
    const approve = () => within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') });
    await userEvent.click(approve());
    await waitFor(() => expect(calls.decide).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(approve()).toBeEnabled());
    expect(within(dialog).getByRole('alert')).toBeVisible();
    expect(cache.getQueryData(approvalKey(second.id))).toMatchObject({ state: 'approved' });
    await userEvent.click(approve());
    await waitFor(() => expect(calls.decide).toHaveBeenCalledTimes(3));
    expect(calls.decide.mock.calls.map(([id]) => id)).toEqual([first.id, second.id, first.id]);
  });

  it(`keeps an already-decided outcome and can deny the remaining selection in ${locale}`, async () => {
    await setLocale(locale);
    calls.decide.mockRejectedValueOnce(new ApiError({ type: 'orch:error:approval.already_decided', code: 'approval.already_decided', status: 409,
      title: 'Already answered', detail: 'Another person answered.', request_id: 'batch-race', current: decided(first, false) }, null));
    show(<Scene items={[first, second]} />);
    const dialog = await open();
    await userEvent.click(within(dialog).getByRole('checkbox', { name: i18n.t('approvals.batch.selectNamed', { name: second.title }) }));
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') }));
    await waitFor(() => expect(cache.getQueryData(approvalKey(first.id))).toMatchObject({ state: 'denied' }));
    await userEvent.click(within(dialog).getByRole('checkbox', { name: i18n.t('approvals.batch.selectNamed', { name: second.title }) }));
    await userEvent.type(within(dialog).getByRole('textbox', { name: i18n.t('approvals.note') }), 'Please ask later');
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('approvals.batch.deny') }));
    await waitFor(() => expect(calls.decide).toHaveBeenLastCalledWith(second.id, { approve: false, note: 'Please ask later' }));
  });

  it(`stops the batch and discards late outcomes after an account change in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<Approval>(); calls.decide.mockReturnValueOnce(pending.promise);
    show(<Scene items={[first, second]} />);
    const dialog = await open();
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') }));
    act(() => { client.session.setState({ userId: 'another-reader', readerVersion: 2 }); });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    await act(() => { pending.resolve(decided(first)); return Promise.resolve(); });
    expect(calls.decide).toHaveBeenCalledTimes(1); expect(calls.settled).not.toHaveBeenCalled();
    expect(cache.getQueryData(approvalKey(first.id))).toBeUndefined();
  });

  it(`stops before the next request when the live connection is lost in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<Approval>(); calls.decide.mockReturnValueOnce(pending.promise);
    const view = show(<Scene items={[first, second]} />);
    const dialog = await open();
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') }));
    view.rerender(<Scene items={[first, second]} live={false} />);
    await act(() => { pending.resolve(decided(first)); return Promise.resolve(); });
    expect(calls.decide).toHaveBeenCalledTimes(1);
    expect(within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') })).toBeDisabled();
  });

  it(`rechecks decision authority before sending the next request in ${locale}`, async () => {
    await setLocale(locale);
    const pending = deferred<Approval>(); calls.decide.mockReturnValueOnce(pending.promise);
    const view = show(<Scene items={[first, second]} />);
    const dialog = await open();
    await userEvent.click(within(dialog).getByRole('button', { name: i18n.t('approvals.batch.approve') }));
    view.rerender(<Scene items={[first, { ...second, can_decide: false }]} />);
    await act(() => { pending.resolve(decided(first)); return Promise.resolve(); });
    expect(calls.decide).toHaveBeenCalledTimes(1);
    expect(within(dialog).getByRole('checkbox', { name: i18n.t('approvals.batch.selectNamed', { name: second.title }) })).toBeDisabled();
  });
}
