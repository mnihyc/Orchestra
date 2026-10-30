/** A requester proposes an end and the decider can accept, change, or clear it. */
import { act, cleanup, fireEvent, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import type * as AccessApi from '@/api/access';
import type * as ApprovalApi from '@/api/approvals';
import { AccessRequest } from '@/features/access/AccessRequest';
import { useRequestEnd } from '@/features/access/RequestEnd';
import { AccessCard } from '@/features/approvals/Access';
import { RequestExport } from '@/features/export/RequestExport';
import { RequestDefault } from '@/features/start/RequestDefault';
import { Where } from '@/features/start/Where';
import type { Approval, MissingAccess, UserRef } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { actionName } from '@/ui/actions';
import { ADA, BEA, TOPIC } from '../unit/general-access-fixtures';
import { environment, options } from '../unit/start-fixtures';
import { show } from './support';

const calls = vi.hoisted(() => ({ request: vi.fn<typeof AccessApi.requestAccess>(), join: vi.fn<typeof AccessApi.requestJoin>(), decide: vi.fn<typeof ApprovalApi.decideApproval>() }));
vi.mock('@/api/access', async (original) => ({ ...await original<typeof AccessApi>(), requestAccess: calls.request, requestJoin: calls.join }));
vi.mock('@/api/approvals', async (original) => ({ ...await original<typeof ApprovalApi>(), decideApproval: calls.decide }));
vi.mock('@/ui/UserChip', () => ({ UserChip: ({ user }: { user: UserRef }) => <span>{user.display_name}</span> }));

const requested = '2100-04-05T12:34:56.123Z';
const approval: Extract<Approval, { kind: 'access' }> = {
  id: 'request', batch_id: 'batch', kind: 'access', state: 'pending', title: 'Review access',
  object: { kind: 'thread', id: 'thread', name: 'Thread' }, node: TOPIC, requester: { user: BEA },
  request: { permissions: ['thread.view'], reason: 'Please', holder: { kind: 'user', user: BEA }, options: null, for_thread: null, expires_at: requested },
  required_permission: 'access.manage', can_decide: true, approver_scope: null, created_at: '2026-10-15T21:31:00Z',
  decided_at: null, decided_automatically: false, decided_by: null, decision: null, expires_at: null, item_id: null, thread_id: null,
};
const missing: MissingAccess = { object: { kind: 'thread', id: 'thread', name: 'Thread', owner: ADA }, permissions: ['thread.view'], options: null, ask: [ADA], ask_more: 0, override: false, closed: null };
const appearance = useAppearance.getState();

beforeEach(() => {
  sessionStorage.clear();
  vi.clearAllMocks();
  calls.request.mockResolvedValue({ batch_id: 'batch', approvals: [approval] });
  calls.join.mockResolvedValue({ batch_id: 'batch', approvals: [approval] });
  calls.decide.mockResolvedValue({ ...approval, state: 'approved' });
});
afterEach(async () => { cleanup(); useAppearance.setState(appearance); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`sends a week from the request time and shows the requested end in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    const user = userEvent.setup();
    show(<AccessRequest missing={[missing]} objectId="thread" fixed />);
    await user.click(screen.getByRole('combobox', { name: i18n.t('access.request.duration') }));
    await user.click(screen.getByRole('option', { name: /1\s*(week|周)/u }));
    const before = Date.now();
    await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
    await waitFor(() => expect(calls.request).toHaveBeenCalledOnce());
    const end = calls.request.mock.calls[0]?.[0].expires_at;
    expect(end).toBeDefined();
    expect(Date.parse(end as string) - before).toBeGreaterThanOrEqual(7 * 24 * 60 * 60 * 1000);
    expect(Date.parse(end as string) - Date.now()).toBeLessThanOrEqual(7 * 24 * 60 * 60 * 1000);
    expect(await screen.findByText(i18n.t('access.request.requestedUntil'))).toBeVisible();
  });

  it(`accepts the exact requested instant and lets a decider clear it in ${locale}`, async () => {
    await setLocale(locale); useAppearance.setState({ locale });
    const user = userEvent.setup();
    const view = show(<AccessCard items={[approval]} settled={new Map()} live onSettled={vi.fn()} />);
    expect(screen.getByRole('combobox', { name: i18n.t('approvals.access.ends') })).toHaveTextContent(i18n.t('access.request.asRequested'));
    expect(view.container.querySelector('[data-requested-end] time')).toHaveAttribute('datetime', requested);
    await user.click(screen.getByRole('button', { name: actionName('approve', approval.title) }));
    await waitFor(() => expect(calls.decide).toHaveBeenCalledWith(approval.id, { approve: true, expires_at: requested }));
    cleanup();
    calls.decide.mockClear();
    show(<AccessCard items={[approval]} settled={new Map()} live onSettled={vi.fn()} />);
    await user.click(screen.getByRole('combobox', { name: i18n.t('approvals.access.ends') }));
    await user.click(screen.getByRole('option', { name: i18n.t('access.request.untilRemoved') }));
    await user.click(screen.getByRole('button', { name: actionName('approve', approval.title) }));
    await waitFor(() => expect(calls.decide).toHaveBeenCalledWith(approval.id, { approve: true }));
  });
}

it('requires a future custom date and sends it as the requested instant', async () => {
  const user = userEvent.setup();
  show(<AccessRequest missing={[missing]} objectId="thread" fixed />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.request.duration') }));
  await user.click(screen.getByRole('option', { name: i18n.t('access.request.chooseDate') }));
  const field = screen.getByLabelText(i18n.t('access.request.endsOn'));
  fireEvent.change(field, { target: { value: '2000-01-01T12:00' } });
  expect(screen.getByRole('button', { name: i18n.t('access.request.send') })).toBeDisabled();
  fireEvent.change(field, { target: { value: '2100-04-05T12:34' } });
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await waitFor(() => expect(calls.request).toHaveBeenCalledOnce());
  expect(calls.request.mock.calls[0]?.[0].expires_at).toBe(new Date('2100-04-05T12:34').toISOString());
});

it.each([['1 day', 1], ['30 days', 30]] as const)('requests %s from the time of sending', async (label, days) => {
  const user = userEvent.setup();
  show(<AccessRequest missing={[missing]} objectId="thread" fixed />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.request.duration') }));
  await user.click(screen.getByRole('option', { name: label, exact: true }));
  const before = Date.now();
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await waitFor(() => expect(calls.request).toHaveBeenCalledOnce());
  const end = Date.parse(calls.request.mock.calls[0]?.[0].expires_at ?? '');
  expect(end).toBeGreaterThanOrEqual(before + days * 24 * 60 * 60 * 1000);
  expect(end).toBeLessThanOrEqual(Date.now() + days * 24 * 60 * 60 * 1000);
});

it('keeps an end on retry and starts a fresh duration for a new request', () => {
  const now = Date.now();
  const clock = vi.spyOn(Date, 'now').mockReturnValue(now);
  const { result } = renderHook(() => useRequestEnd());
  act(() => result.current.setDuration('week'));
  const first = result.current.end(0);
  clock.mockReturnValue(now + 60_000);
  expect(result.current.end(0)).toBe(first);
  expect(result.current.end(1)).toBe(new Date(now + 60_000 + 7 * 24 * 60 * 60 * 1000).toISOString());
  clock.mockRestore();
});

it('sends the same proposed end when asking to join a group chat', async () => {
  const user = userEvent.setup();
  const chat = { ...missing, object: { ...missing.object, kind: 'chat', id: 'group' }, permissions: [] };
  show(<AccessRequest missing={[chat]} objectId="group" fixed privateReference membership sharedLabel="Shared group" />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.request.duration') }));
  await user.click(screen.getByRole('option', { name: /1\s*(week|周)/u }));
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await waitFor(() => expect(calls.join).toHaveBeenCalledOnce());
  expect(calls.join.mock.calls[0]?.[0]).toBe('group');
  expect(calls.join.mock.calls[0]?.[1]).toMatchObject({ reason: '', label: 'Shared group', expires_at: expect.any(String) });
});

it.each(['export', 'defaults'] as const)('shows the proposed end after sending the %s request', async (kind) => {
  const user = userEvent.setup();
  show(kind === 'export'
    ? <RequestExport target={{ kind: 'thread', id: 'thread', name: 'Thread', owner: ADA }} open onClose={vi.fn()} />
    : <RequestDefault place={{ id: TOPIC.id, kind: 'topic', name: 'Release', parentId: 'project', archived: false, status: 'open', folder: false }} owner={ADA} open onClose={vi.fn()} />);
  await user.click(screen.getByRole('button', { name: i18n.t(kind === 'export' ? 'export.request.send' : 'start.request.send') }));
  await waitFor(() => expect(document.querySelector('[data-requested-end] time')).toHaveAttribute('datetime', requested));
});

it('requires the decider to change a requested end that has passed', () => {
  const expired = { ...approval, request: { ...approval.request, expires_at: '2000-01-01T00:00:00Z' } };
  show(<AccessCard items={[expired]} settled={new Map()} live onSettled={vi.fn()} />);
  expect(screen.getByText(i18n.t('access.request.futureDate'))).toBeVisible();
  expect(screen.getByRole('button', { name: actionName('approve', approval.title) })).toBeDisabled();
});

it('shows the proposed end to a reader who cannot decide the request', () => {
  const view = show(<AccessCard items={[{ ...approval, can_decide: false }]} settled={new Map()} live onSettled={vi.fn()} />);
  expect(view.container.querySelector('[data-requested-end] time')).toHaveAttribute('datetime', requested);
  expect(screen.queryByRole('combobox', { name: i18n.t('approvals.access.ends') })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: actionName('approve', approval.title) })).not.toBeInTheDocument();
  view.rerender(<AccessCard items={[approval]} settled={new Map()} live onSettled={vi.fn()} />);
  expect(screen.getByRole('combobox', { name: i18n.t('approvals.access.ends') })).toHaveTextContent(i18n.t('access.request.asRequested'));
});

it('lets a decider replace the proposed end with a different date', async () => {
  const user = userEvent.setup();
  show(<AccessCard items={[approval]} settled={new Map()} live onSettled={vi.fn()} />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('approvals.access.ends') }));
  await user.click(screen.getByRole('option', { name: i18n.t('approvals.access.customEnd') }));
  const field = screen.getByLabelText(i18n.t('approvals.access.endsOn'));
  fireEvent.change(field, { target: { value: '2100-04-06T13:45' } });
  await user.click(screen.getByRole('button', { name: actionName('approve', approval.title) }));
  await waitFor(() => expect(calls.decide).toHaveBeenCalledWith(approval.id, { approve: true, expires_at: new Date('2100-04-06T13:45').toISOString() }));
});

it('offers the same duration when requesting Use from the new-thread picker', async () => {
  const blocked = { ...environment('blocked', { usable: false }), reason: 'environment_use_required' };
  const user = userEvent.setup();
  show(<Where compact place={{ id: 'topic', name: 'Release', kind: 'topic', parentId: 'project', archived: false, status: 'open', folder: false }}
    homeId="acme" machines={[]} options={options({ environments: [blocked], can_create: { allowed: false, missing: [] } })} value={null} onChange={vi.fn()} />);
  await user.click(screen.getByRole('combobox', { name: i18n.t('start.where.title') }));
  await user.click(await screen.findByRole('option', { name: blocked.environment.name }));
  await user.click(screen.getByRole('combobox', { name: i18n.t('access.request.duration') }));
  await user.click(screen.getByRole('option', { name: /1\s*(week|周)/u }));
  await user.click(screen.getByRole('button', { name: i18n.t('access.request.send') }));
  await waitFor(() => expect(calls.request).toHaveBeenCalledOnce());
  expect(calls.request.mock.calls[0]?.[0].items[0]).toEqual({ object: { kind: 'environment', id: 'blocked' }, permissions: ['environment.use'] });
  expect(calls.request.mock.calls[0]?.[0].expires_at).toBeDefined();
});
