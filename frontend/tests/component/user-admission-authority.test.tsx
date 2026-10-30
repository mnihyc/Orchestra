/** Every accepted user source closes the authority interval before subscription, even at the same Inbox cursor. */
import { QueryObserver } from '@tanstack/react-query';
import { act, cleanup, render, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { outsideQuery } from '@/api/away';
import { queryClient } from '@/api/query';
import { ADMIN_QUERY } from '@/api/queryAuthority';
import type { AdminMode, Me, NotificationPage, NotificationSummaryGetReply } from '@/generated/api-types';
import { feeds, socket, useUserFeed } from '@/realtime/live';
import type { FeedListener } from '@/realtime/socket';
import { userWriter } from '@/realtime/stores/user';
import { me, meta } from '../doubles/fixtures';
import type * as NotificationsApi from '@/api/notifications';
import type * as AuthApi from '@/api/auth';
import type * as AdminStateApi from '@/api/adminState';
import type * as AdminModeApi from '@/api/adminMode';

const calls = vi.hoisted(() => ({ person: vi.fn<() => Promise<Me>>(), mode: vi.fn<() => Promise<AdminMode>>(),
  summary: vi.fn<() => Promise<NotificationSummaryGetReply>>(), page: vi.fn<() => Promise<NotificationPage>>(),
  meta: vi.fn(() => Promise.resolve()) }));
vi.mock('@/api/auth', async (original) => ({ ...await original<typeof AuthApi>(), meQuery: { queryKey: ['me'], queryFn: calls.person, staleTime: Infinity },
  fetchShellAuthority: () => Promise.all([calls.person(), calls.mode()]).then(([person, admin_mode]) => ({ person, admin_mode, orgs: [], meta: meta() })),
}));
vi.mock('@/api/adminMode', async (original) => ({ ...await original<typeof AdminModeApi>(), adminModeQuery: { queryKey: ['admin-mode'], queryFn: calls.mode, staleTime: Infinity } }));
vi.mock('@/api/adminState', async (original) => ({ ...await original<typeof AdminStateApi>(), repairMeta: calls.meta }));
vi.mock('@/api/notifications', async (original) => ({ ...await original<typeof NotificationsApi>(), fetchSummary: calls.summary, fetchNotifications: calls.page }));
const previous = client.session.getState();
const mode: AdminMode = { as_admin: true, available: true };
const summary: NotificationSummaryGetReply = { seq: 7, unread: 0, approvals_pending: 0, outside: { scope: null, pause_until: null, ways: [] } };
const subscriptions: FeedListener[] = [];
function UserSource() { useUserFeed(false); return null; }
function admit(id: number) {
  const listener = subscriptions.at(-1);
  if (listener === undefined) throw new Error('The user source requested admission');
  act(() => listener.onFrame({ op: 'subscribed', id, feed: 'user', seq: summary.seq, mode: 'delta' }));
}
beforeEach(() => {
  feeds.clear(); queryClient.clear(); userWriter.clear(); subscriptions.length = 0;
  calls.person.mockReset().mockResolvedValue(me(1, 'person', 'Person', { is_system_admin: true }));
  calls.mode.mockReset().mockResolvedValue(mode); calls.summary.mockReset().mockResolvedValue(summary);
  calls.page.mockReset().mockResolvedValue({ items: [], next_cursor: null }); calls.meta.mockClear();
  queryClient.setQueryData(['me'], me(1, 'person', 'Person', { is_system_admin: true }));
  queryClient.setQueryData(['admin-mode'], mode); queryClient.setQueryData(outsideQuery().queryKey, summary);
  vi.spyOn(socket, 'subscribe').mockImplementation((feed, listener) => {
    if (feed !== 'user') throw new Error('This fixture owns only the personal source');
    subscriptions.push(listener); return { unsubscribe: vi.fn() };
  });
});
afterEach(() => { cleanup(); feeds.clear(); queryClient.clear(); userWriter.clear(); client.session.setState(previous); vi.restoreAllMocks(); });

it.each(['initial', 'resumed'] as const)('retakes current standing at %s admission without an Inbox event, page read or background polling', async (kind) => {
  const key = ['admin', 'settings']; queryClient.setQueryData(key, 'Former private settings');
  const readSettings = vi.fn(() => Promise.resolve('System settings'));
  const leave = new QueryObserver(queryClient, { queryKey: key, queryFn: readSettings, staleTime: Infinity, ...ADMIN_QUERY }).subscribe(() => undefined);
  const view = render(<UserSource />);
  try {
    await waitFor(() => expect(subscriptions).toHaveLength(1));
    if (kind === 'resumed') {
      // The first admission is unchanged; a later reconnect carries no replayed authority event.
      admit(1);
      await waitFor(() => expect(calls.person).toHaveBeenCalledOnce());
      expect(calls.mode).toHaveBeenCalledOnce(); expect(readSettings).not.toHaveBeenCalled();
      calls.person.mockClear(); calls.mode.mockClear(); calls.meta.mockClear();
    }
    calls.person.mockResolvedValue(me(1, 'person', 'Person', { is_system_admin: false }));
    admit(kind === 'initial' ? 1 : 2);
    await waitFor(() => expect(queryClient.getQueryData<Me>(['me'])?.is_system_admin).toBe(false));
    expect(queryClient.getQueryData(['admin-mode'])).toEqual(mode);
    expect(queryClient.getQueryData(key)).toBeUndefined(); expect(readSettings).not.toHaveBeenCalled();
    expect(calls.person).toHaveBeenCalledOnce(); expect(calls.mode).toHaveBeenCalledOnce();
    expect(calls.page).not.toHaveBeenCalled();
    // An unchanged admitted connection schedules no recurring authority supplier.
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 60)); });
    expect(calls.person).toHaveBeenCalledOnce(); expect(calls.mode).toHaveBeenCalledOnce();
  } finally { leave(); view.unmount(); }
});
