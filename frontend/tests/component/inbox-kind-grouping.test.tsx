/** Kind grouping joins later rows, keeps filters and saves through the existing personal view state. */
import { cleanup, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type * as Router from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Personal from '@/api/personal';
import { ApiError } from '@/api/client';
import { Inbox } from '@/features/inbox/Inbox';
import type { Notification } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { userWriter, useUserStore } from '@/realtime/stores/user';
import { show } from './support';

const calls = vi.hoisted(() => ({ save: vi.fn(), navigate: vi.fn(), read: vi.fn() }));
vi.mock('@/api/personal', async (load) => ({ ...await load<typeof Personal>(),
  meStateQuery: { queryKey: ['kind-grouping-state'], queryFn: () => Promise.resolve({ inbox: { group_by: 'kind', retained: true } }) },
  setMeState: calls.save,
}));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useNavigate: () => calls.navigate }));
vi.mock('@/features/inbox/read', () => ({ markNotificationsRead: calls.read }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof import('@/realtime/live')>(), useUserFeed: () => undefined }));
vi.mock('@/features/settings/personal/away/live', () => ({ useAwayLive: () => undefined }));
vi.mock('@/features/settings/personal/away/Pause', () => ({ PauseControls: () => null }));
vi.mock('@/features/inbox/presentation', async (load) => ({ ...await load<typeof import('@/features/inbox/presentation')>(), useNotificationLabels: () => new Map() }));
vi.mock('@/shell/screen', () => ({ useScreenHeading: () => undefined }));
vi.mock('@/features/help/Hint', () => ({ Hint: () => null }));

const before = useUserStore.getState();
function row(id: string, kind: Notification['kind'], read = false): Notification {
  return { id, kind, title: `Notice ${id}`, body: '', node: null, thread_id: null, approval_id: null, chat_id: null,
    url: '/settings/notifications', data: {}, created_at: `2026-10-15T12:00:0${id}Z`, read_at: read ? '2026-10-15T13:00:00Z' : null,
    inbox_seq: Number(id), group: null, waiting_for_me: false };
}
beforeEach(() => { vi.clearAllMocks(); calls.save.mockResolvedValue(undefined); calls.read.mockResolvedValue(undefined); });
afterEach(async () => { cleanup(); useUserStore.setState(before); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`restores and saves kind grouping without losing the other view state in ${locale}`, async () => {
    await setLocale(locale);
    userWriter.reset([row('5', 'turn.completed'), row('4', 'job.finished'), row('3', 'turn.completed'), row('2', 'future.one' as Notification['kind']), row('1', 'future.two' as Notification['kind'])], { unread: 5, approvalsPending: 0 }, 5, null);
    show(<Inbox filter="all" onFilter={() => undefined} />);
    expect(await screen.findByRole('heading', { name: i18n.t('inbox.kinds.turn_completed'), level: 2 })).toBeVisible();
    expect(screen.getAllByRole('heading', { name: i18n.t('inbox.otherKinds'), level: 2 })).toHaveLength(1);
    expect(screen.getAllByRole('listitem')).toHaveLength(5);
    const selector = screen.getByRole('combobox', { name: i18n.t('away.inbox.grouping') });
    expect(selector).toHaveTextContent(i18n.t('choiceCatalog.inboxGrouping.kind.name'));
    await userEvent.click(selector);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.inboxGrouping.day.name') }));
    await waitFor(() => expect(calls.save).toHaveBeenCalledWith('inbox', { group_by: 'day', retained: true }));
    expect(screen.queryByRole('heading', { name: i18n.t('inbox.kinds.turn_completed'), level: 2 })).not.toBeInTheDocument();
    await userEvent.click(selector);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.inboxGrouping.kind.name') }));
    expect(await screen.findByRole('heading', { name: i18n.t('inbox.kinds.turn_completed'), level: 2 })).toBeVisible();
    expect(screen.getAllByRole('listitem')).toHaveLength(5);
  });

  it(`groups only the unread filtered rows in ${locale}`, async () => {
    await setLocale(locale);
    userWriter.reset([row('3', 'turn.completed', true), row('2', 'job.finished')], { unread: 1, approvalsPending: 0 }, 3, null);
    show(<Inbox filter="unread" onFilter={() => undefined} />);
    expect(await screen.findByRole('heading', { name: i18n.t('inbox.kinds.job_finished'), level: 2 })).toBeVisible();
    expect(screen.queryByRole('heading', { name: i18n.t('inbox.kinds.turn_completed'), level: 2 })).not.toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
  });

  it(`retries a failed view save without changing the chosen grouping or its other state in ${locale}`, async () => {
    await setLocale(locale);
    calls.save.mockRejectedValueOnce(new Error('View save temporarily unavailable'));
    userWriter.reset([row('1', 'turn.completed')], { unread: 1, approvalsPending: 0 }, 1, null);
    show(<Inbox filter="all" onFilter={() => undefined} />);
    const selector = await screen.findByRole('combobox', { name: i18n.t('away.inbox.grouping') });
    await waitFor(() => expect(selector).not.toBeDisabled());
    await userEvent.click(selector);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.inboxGrouping.day.name') }));
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('away.inbox.viewSaveFailed'));
    expect(selector).toHaveTextContent(i18n.t('choiceCatalog.inboxGrouping.day.name'));
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
    await userEvent.click(screen.getByRole('button', { name: i18n.t('common.tryAgain') }));
    await waitFor(() => expect(calls.save).toHaveBeenCalledTimes(2));
    expect(calls.save.mock.calls).toEqual([
      ['inbox', { group_by: 'day', retained: true }],
      ['inbox', { group_by: 'day', retained: true }],
    ]);
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull());
    expect(selector).toHaveTextContent(i18n.t('choiceCatalog.inboxGrouping.day.name'));
    expect(screen.getAllByRole('listitem')).toHaveLength(1);
  });

  it(`keeps a permanent view refusal without offering a temporary retry in ${locale}`, async () => {
    await setLocale(locale);
    calls.save.mockRejectedValueOnce(new ApiError({ type: 'orch:error:request.invalid', code: 'request.invalid', status: 400, title: '', detail: '', request_id: '' }, null));
    userWriter.reset([row('1', 'turn.completed')], { unread: 1, approvalsPending: 0 }, 1, null);
    show(<Inbox filter="all" onFilter={() => undefined} />);
    const selector = await screen.findByRole('combobox', { name: i18n.t('away.inbox.grouping') });
    await waitFor(() => expect(selector).not.toBeDisabled());
    await userEvent.click(selector);
    await userEvent.click(screen.getByRole('option', { name: i18n.t('choiceCatalog.inboxGrouping.day.name') }));
    expect(await screen.findByRole('alert')).toHaveTextContent(i18n.t('away.inbox.viewSaveFailed'));
    expect(screen.queryByRole('button', { name: i18n.t('common.tryAgain') })).toBeNull();
    expect(selector).toHaveTextContent(i18n.t('choiceCatalog.inboxGrouping.day.name'));
    expect(calls.save).toHaveBeenCalledOnce();
  });
}
