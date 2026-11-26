/** Only a displayed conversation may consume its thread notifications. */
import { Activity } from 'react';
import { act, cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useReadWhenOpened } from '@/features/inbox/Counters';
import { userWriter, useUserStore } from '@/realtime/stores/user';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { feeds, reloadUserFeed, socket, useUserFeed } from '@/realtime/live';
import { queryClient } from '@/api/query';
import type { FeedListener } from '@/realtime/socket';
import { READ_RETRY_PAUSES_MS } from '@/lib/requests';

const mark = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('@/features/inbox/read', () => ({ markNotificationsRead: mark }));
const previous = useUserStore.getState();
function Conversation() { useReadWhenOpened('visible-thread'); return null; }
function Frame({ shown }: { shown: boolean }) { return <Activity mode={shown ? 'visible' : 'hidden'}><Conversation /></Activity>; }
afterEach(() => { cleanup(); useUserStore.setState(previous); mark.mockClear(); });

it('silently replaces a retired Inbox page in the actual personal feed and resumes its current read', async () => {
  vi.useFakeTimers(); const random = vi.spyOn(Math, 'random').mockReturnValue(0);
  feeds.clear(); userWriter.clear(); queryClient.clear();
  let listener: FeedListener | undefined, finish!: (reply: Response) => void, pages = 0;
  const subscribed = vi.spyOn(socket, 'subscribe').mockImplementation((_feed, next) => { listener = next; return { unsubscribe: () => undefined }; });
  vi.stubGlobal('fetch', (input: RequestInfo | URL) => {
    const path = new URL(input instanceof Request ? input.url : String(input)).pathname;
    if (path.endsWith('/notifications/summary')) return Promise.resolve(Response.json({ unread: 0, approvals_pending: 0, seq: 9 }));
    if (!path.endsWith('/notifications')) return Promise.reject(new Error(`Unexpected Inbox read ${path}`));
    pages += 1;
    return pages === 1 ? new Promise<Response>((resolve) => { finish = resolve; }) : Promise.resolve(Response.json({ items: [], next_cursor: null }));
  });
  const failed = vi.spyOn(userWriter, 'failed');
  function Inbox() { useUserFeed(); return null; }
  const view = render(<Inbox />);
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(0); }); expect(pages).toBe(1);
    await act(async () => { reloadUserFeed(); await vi.advanceTimersByTimeAsync(0); });
    if (listener === undefined) throw new Error('The actual Inbox must own its personal source');
    await act(async () => {
      listener?.onFrame({ op: 'resync', id: 1, feed: 'user', reload: true });
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(failed).not.toHaveBeenCalled(); expect(useUserStore.getState().error).toBeNull();
    await act(async () => { await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]); });
    expect(pages).toBe(2); expect(useUserStore.getState()).toMatchObject({ loaded: true, lastSeq: 9, error: null });
    await act(async () => { finish(Response.json({ items: [], next_cursor: 'retired-page' })); await vi.advanceTimersByTimeAsync(0); });
    expect(useUserStore.getState().nextCursor).toBeNull(); expect(failed).not.toHaveBeenCalled();
  } finally { view.unmount(); feeds.clear(); queryClient.clear(); failed.mockRestore(); subscribed.mockRestore(); random.mockRestore(); vi.unstubAllGlobals(); vi.useRealTimers(); }
});

it('keeps notification state unchanged when a conversation is shown again', async () => {
  useUserStore.setState({ loaded: true, rows: new Map() });
  const view = render(<Frame shown={false} />);
  await act(async () => { await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<Frame shown />);
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<Frame shown={false} />); mark.mockClear();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
});

it('does not write a read mark merely because an admitted thread has unread notifications', () => {
  useUserStore.setState({ loaded: false, rows: new Map(), counters: { unread: 2, approvalsPending: 0 }, reachedEnd: false });
  render(<Frame shown />);
  expect(mark).not.toHaveBeenCalled();
  expect(useUserStore.getState().loaded).toBe(false);
  expect(useUserStore.getState().rows.size).toBe(0);
});

it('waits for frame admission and stops reading when that admission is lost', async () => {
  useUserStore.setState({ loaded: false, rows: new Map(), counters: { unread: 2, approvalsPending: 0 }, reachedEnd: false });
  const view = render(<FrameReadiness.Provider value={{ ready: false, pending: null }}><Frame shown /></FrameReadiness.Provider>);
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<FrameReadiness.Provider value={{ ready: true, pending: null }}><Frame shown /></FrameReadiness.Provider>);
  expect(mark).not.toHaveBeenCalled();
  view.rerender(<FrameReadiness.Provider value={{ ready: false, pending: null }}><Frame shown /></FrameReadiness.Provider>);
  mark.mockClear();
  await act(async () => { useUserStore.setState({ rows: new Map() }); document.dispatchEvent(new Event('visibilitychange')); await Promise.resolve(); });
  expect(mark).not.toHaveBeenCalled();
});
