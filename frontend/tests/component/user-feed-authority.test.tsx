/** The actual shared user feed and visible Inbox hook own one current first-page query. */
import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Notifications from '@/api/notifications';
import { outsideQuery } from '@/api/away';
import { queryClient } from '@/api/query';
import type { Notification, NotificationPage, NotificationSummaryGetReply } from '@/generated/api-types';
import { feeds, socket, useUserFeed } from '@/realtime/live';
import type { FeedListener } from '@/realtime/socket';
import { userWriter, useUserStore } from '@/realtime/stores/user';
import type * as ActsApi from '@/api/acts';

const calls = vi.hoisted(() => ({ page: vi.fn<typeof Notifications.fetchNotifications>(), summary: vi.fn<typeof Notifications.fetchSummary>() }));
vi.mock('@/api/notifications', async (original) => ({ ...await original<typeof Notifications>(), fetchNotifications: calls.page, fetchSummary: calls.summary }));
// Authority capsule lifecycle has its own integration tests; this fixture isolates the Inbox supplier.
vi.mock('@/api/acts', async (original) => ({ ...await original<typeof ActsApi>(), applyChanges: vi.fn(() => Promise.resolve()) }));
const subscriptions: { listener: FeedListener; leave: ReturnType<typeof vi.fn> }[] = [];
const summary: NotificationSummaryGetReply = { seq: 1, unread: 0, approvals_pending: 0, outside: { scope: null, pause_until: null, ways: [] } };
function row(id: string): Notification {
  return { id, kind: 'turn.completed', title: id, body: '', node: null, thread_id: null, approval_id: null,
    chat_id: null, url: '/inbox', data: {}, created_at: '2026-10-23T00:00:00Z', read_at: null,
    inbox_seq: 1, group: null, waiting_for_me: false };
}
function pending() {
  let resolve!: (page: NotificationPage) => void;
  const promise = new Promise<NotificationPage>((done) => { resolve = done; });
  return { promise, resolve };
}
function InboxOwner({ rows = true }: { rows?: boolean }) { useUserFeed(rows); return null; }
function resetAuthority() {
  const subscription = subscriptions[0];
  if (subscription === undefined) throw new Error('The user source is subscribed');
  act(() => subscription.listener.onFrame({ op: 'resync', id: 1, feed: 'user', reload: true }));
}
beforeEach(() => {
  feeds.clear(); queryClient.clear(); userWriter.clear(); subscriptions.length = 0;
  calls.page.mockReset(); calls.summary.mockReset().mockResolvedValue({ ...summary, seq: 2 });
  queryClient.setQueryData(outsideQuery().queryKey, summary);
  vi.spyOn(socket, 'subscribe').mockImplementation((feed, listener) => {
    if (feed !== 'user') throw new Error('This fixture owns only the user feed');
    const leave = vi.fn(); subscriptions.push({ listener, leave }); return { unsubscribe: leave };
  });
});
afterEach(() => { cleanup(); feeds.clear(); queryClient.clear(); userWriter.clear(); vi.restoreAllMocks(); });

it('retires the pending old first page before the replacement feed reads and cannot revive it through the visible hook', async () => {
  const old = pending(), fresh = pending();
  calls.page.mockImplementation(() => calls.page.mock.calls.length === 1 ? old.promise : fresh.promise);
  const view = render(<InboxOwner />);
  try {
    await waitFor(() => { expect(calls.page).toHaveBeenCalledOnce(); expect(subscriptions).toHaveLength(1); });
    const oldSignal = calls.page.mock.calls[0]?.[1];
    resetAuthority();
    await waitFor(() => expect(calls.page).toHaveBeenCalledTimes(2));
    expect(oldSignal?.aborted).toBe(true);
    expect(queryClient.getQueryData(['inbox', 'first-page'])).toBeUndefined();
    await act(async () => { old.resolve({ items: [row('former-private')], next_cursor: 'former-cursor' }); await Promise.resolve(); });
    expect(useUserStore.getState().rows.size).toBe(0);
    expect(useUserStore.getState().error).toBeNull();
    await act(async () => { fresh.resolve({ items: [row('current')], next_cursor: null }); await Promise.resolve(); });
    await waitFor(() => expect(useUserStore.getState().rows.has('current')).toBe(true));
    expect(useUserStore.getState().rows.has('former-private')).toBe(false);
    expect(useUserStore.getState().nextCursor).toBeNull();
    expect(calls.page).toHaveBeenCalledTimes(2); expect(calls.summary).toHaveBeenCalledOnce();
    expect(subscriptions).toHaveLength(2);
  } finally { old.resolve({ items: [], next_cursor: null }); fresh.resolve({ items: [], next_cursor: null }); view.unmount(); }
});

it('retires a cached former Inbox page while the counter-only user feed opens no hidden page read', async () => {
  queryClient.setQueryData(['inbox', 'first-page'], { items: [row('hidden-private')], next_cursor: 'hidden-cursor' });
  const view = render(<InboxOwner rows={false} />);
  try {
    await waitFor(() => expect(subscriptions).toHaveLength(1));
    resetAuthority();
    await waitFor(() => expect(subscriptions).toHaveLength(2));
    expect(queryClient.getQueryData(['inbox', 'first-page'])).toBeUndefined();
    expect(useUserStore.getState().rows.size).toBe(0);
    expect(calls.page).not.toHaveBeenCalled(); expect(calls.summary).toHaveBeenCalledOnce();
  } finally { view.unmount(); }
});

it('cancels a page whose last visible owner left without reading another hidden page', async () => {
  const old = pending(); calls.page.mockReturnValue(old.promise);
  const view = render(<InboxOwner />);
  try {
    await waitFor(() => { expect(calls.page).toHaveBeenCalledOnce(); expect(subscriptions).toHaveLength(1); });
    const oldSignal = calls.page.mock.calls[0]?.[1];
    view.rerender(<InboxOwner rows={false} />);
    resetAuthority();
    await waitFor(() => expect(subscriptions).toHaveLength(2));
    expect(oldSignal?.aborted).toBe(true);
    await act(async () => { old.resolve({ items: [row('departed-private')], next_cursor: null }); await Promise.resolve(); });
    expect(useUserStore.getState().rows.size).toBe(0);
    expect(useUserStore.getState().error).toBeNull();
    expect(calls.page).toHaveBeenCalledOnce(); expect(calls.summary).toHaveBeenCalledOnce();
  } finally { old.resolve({ items: [], next_cursor: null }); view.unmount(); }
});
