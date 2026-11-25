/** The HTTP baseline follows accepted personal-source admission, including reconnect and reload. */
import { expect, it, vi } from 'vitest';
import { admittedOrOffline, dispatchPersonalEvent, followPersonalSource, holdPersonalEvents, retirePersonalEvents } from '@/realtime/personalSource';
import type { FeedListener, RealtimeSocket, SocketStatus } from '@/realtime/socket';
import { createStore } from 'zustand/vanilla';
import { supersededRead } from '@/api/reads';
import { READ_RETRY_PAUSES_MS } from '@/lib/requests';

function setup() {
  const listeners: FeedListener[] = [];
  const subscribe = vi.fn((_feed: string, listener: FeedListener) => { listeners.push(listener); listener.onSubscribe?.(listeners.length); return { unsubscribe: vi.fn() }; });
  const load = vi.fn(() => Promise.resolve({ seq: 9, complete: true, data: 'baseline' }));
  const admitted = vi.fn(); const apply = vi.fn();
  const source = followPersonalSource({ subscribe } as unknown as RealtimeSocket, { feed: 'user', load, apply, reset: vi.fn(), read: () => 'row', subscribed: admitted }, vi.fn());
  const accept = (listener = listeners.at(-1)!) => listener.onFrame({ op: 'subscribed', feed: 'user', id: 1, seq: 9, mode: 'delta' });
  return { source, listeners, load, apply, admitted, accept };
}

it('retries a superseded personal baseline without failing its still-current source admission', async () => {
  vi.useFakeTimers(); const random = vi.spyOn(Math, 'random').mockReturnValue(0);
  let listener: FeedListener | undefined;
  const failed = vi.fn(), reset = vi.fn();
  const load = vi.fn().mockRejectedValueOnce(supersededRead()).mockResolvedValue({ seq: 9, complete: true, data: 'Current baseline' });
  const source = followPersonalSource({ subscribe: (_feed: string, next: FeedListener) => { listener = next; return { unsubscribe: () => undefined }; } } as unknown as RealtimeSocket,
    { feed: 'user', load, failed, reset, apply: vi.fn(), read: () => '' }, vi.fn());
  try {
    listener?.onFrame({ op: 'subscribed', id: 1, feed: 'user', mode: 'reload', seq: 0 });
    await vi.advanceTimersByTimeAsync(0); expect(failed).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
    expect(load).toHaveBeenCalledTimes(2); expect(reset).toHaveBeenCalledWith('Current baseline', 9); expect(failed).not.toHaveBeenCalled();
  } finally { source.stop(); random.mockRestore(); vi.useRealTimers(); }
});

it('joins without an HTTP pre-read and opens one baseline only after the accepted cut', async () => {
  const { source, load, accept, admitted } = setup();
  const baseline = vi.fn(); const ready = source.ready.then(baseline);
  await Promise.resolve(); expect(load).not.toHaveBeenCalled(); expect(baseline).not.toHaveBeenCalled();
  accept(); await ready; expect(admitted).toHaveBeenCalledOnce(); expect(baseline).toHaveBeenCalledOnce();
  source.stop();
});

it('a reload is not admission and rejoins at the repaired native cursor', async () => {
  const { source, listeners, load, accept, admitted } = setup();
  listeners[0]!.onFrame({ op: 'subscribed', feed: 'user', id: 1, seq: 9, mode: 'reload' });
  expect(admitted).not.toHaveBeenCalled(); await vi.waitFor(() => expect(listeners).toHaveLength(2));
  expect(load).toHaveBeenCalledOnce(); expect(listeners[1]!.since()).toBe(9);
  accept(); await source.ready; expect(admitted).toHaveBeenCalledOnce(); source.stop();
});

it('a rejoin hears the news of the subscription it replaces until the new one is in place and the server has ended the former', async () => {
  // A machine's enrollment resets the person's feed, and its notice that the machine is online comes while the feed is
  // being joined again: sent on the former subscription before the server has the release, or by nobody when the server
  // released that one before it had begun the next. News is never replayed, so the former one is held until then.
  const listeners: FeedListener[] = [];
  const left: number[] = [];
  const subscribe = vi.fn((_feed: string, listener: FeedListener) => {
    const number = listeners.push(listener); listener.onSubscribe?.(number);
    return { unsubscribe: () => { left.push(number); } };
  });
  const other = vi.fn(), apply = vi.fn(), failed = vi.fn(), closedFeed = vi.fn();
  const load = vi.fn(() => Promise.resolve({ seq: 9, complete: true, data: 'baseline' }));
  const source = followPersonalSource({ subscribe } as unknown as RealtimeSocket,
    { feed: 'user', load, apply, reset: vi.fn(), failed, closed: closedFeed, read: () => 'row', other }, vi.fn());
  try {
    listeners[0]!.onFrame({ op: 'subscribed', feed: 'user', id: 1, seq: 8, mode: 'delta' });
    listeners[0]!.onFrame({ op: 'resync', feed: 'user', id: 1, reload: true });
    await vi.waitFor(() => expect(listeners).toHaveLength(2));
    expect(left).toEqual([]);
    const online = { op: 'event', feed: 'user', id: 1, data: { what: 'enrollment', receipt: 'r1' } } as const;
    listeners[0]!.onFrame(online);
    expect(other).toHaveBeenCalledWith(online);
    // Only its news: the source's cursor, its repairs and its end are the new subscription's.
    listeners[0]!.onFrame({ op: 'sync', feed: 'user', id: 1, seq: 12, prev: 8, data: {} });
    listeners[0]!.onFrame({ op: 'resync', feed: 'user', id: 1, reload: true });
    listeners[0]!.onFrame({ op: 'closed', feed: 'user', id: 1, reason: 'revoked' });
    expect(apply).not.toHaveBeenCalled(); expect(failed).not.toHaveBeenCalled(); expect(closedFeed).not.toHaveBeenCalled();
    expect(load).toHaveBeenCalledOnce();
    listeners[1]!.onFrame({ op: 'subscribed', feed: 'user', id: 2, seq: 9, mode: 'delta' });
    expect(left).toEqual([1]);
    await source.ready;
    // What the server had sent on the former one before it heard of the release still arrives, and is heard as news.
    const late = { op: 'event', feed: 'user', id: 1, data: { what: 'enrollment', receipt: 'r1', status: {} } } as const;
    listeners[0]!.afterRelease?.(late);
    expect(other).toHaveBeenLastCalledWith(late);
    listeners[0]!.afterRelease?.({ op: 'sync', feed: 'user', id: 1, seq: 13, prev: 9, data: {} });
    expect(apply).not.toHaveBeenCalled();
    const later = { op: 'event', feed: 'user', id: 2, data: { what: 'enrollment', receipt: 'r2' } } as const;
    listeners[1]!.onFrame(later);
    expect(other).toHaveBeenLastCalledWith(later);
  } finally { source.stop(); }
  expect(left).toEqual([1, 2]);
  // A stopped source hears nothing late.
  const calls = other.mock.calls.length;
  listeners[1]!.afterRelease?.({ op: 'event', feed: 'user', id: 2, data: { what: 'enrollment', receipt: 'r3' } });
  expect(other).toHaveBeenCalledTimes(calls);
});

it('a rejoin that is refused lets the former subscription go too, and stopping leaves both', async () => {
  const listeners: FeedListener[] = [];
  const left: number[] = [];
  const subscribe = vi.fn((_feed: string, listener: FeedListener) => {
    const number = listeners.push(listener); listener.onSubscribe?.(number);
    return { unsubscribe: () => { left.push(number); } };
  });
  const load = vi.fn(() => Promise.resolve({ seq: 9, complete: true, data: 'baseline' }));
  const source = followPersonalSource({ subscribe } as unknown as RealtimeSocket,
    { feed: 'user', load, apply: vi.fn(), reset: vi.fn(), failed: vi.fn(), read: () => 'row', other: vi.fn() }, vi.fn());
  listeners[0]!.onFrame({ op: 'subscribed', feed: 'user', id: 1, seq: 8, mode: 'delta' });
  listeners[0]!.onFrame({ op: 'resync', feed: 'user', id: 1, reload: true });
  await vi.waitFor(() => expect(listeners).toHaveLength(2));
  listeners[1]!.onFrame({ op: 'error', id: 2, code: 'too_many_subscriptions', detail: 'a connection follows at most 64 feeds' });
  expect(left).toEqual([1]);
  listeners[0]!.onFrame({ op: 'resync', feed: 'user', id: 1, reload: true });
  source.stop();
  expect(left).toEqual([1, 2]);
  // Stopping while a rejoin waits for its acceptance leaves the former subscription as well.
  const waiting: FeedListener[] = [];
  const gone: number[] = [];
  const second = followPersonalSource({ subscribe: (_feed: string, listener: FeedListener) => {
    const number = waiting.push(listener); return { unsubscribe: () => { gone.push(number); } };
  } } as unknown as RealtimeSocket, { feed: 'user', load, apply: vi.fn(), reset: vi.fn(), read: () => 'row' }, vi.fn());
  waiting[0]!.onFrame({ op: 'subscribed', feed: 'user', id: 1, seq: 8, mode: 'delta' });
  waiting[0]!.onFrame({ op: 'resync', feed: 'user', id: 1, reload: true });
  await vi.waitFor(() => expect(waiting).toHaveLength(2));
  second.stop();
  expect(gone.sort()).toEqual([1, 2]);
});

it('a new wire attempt waits for its own admission and does not share the former ready promise', async () => {
  const { source, listeners, accept, admitted } = setup();
  accept(); await source.ready; const former = source.ready;
  listeners[0]!.onSubscribe?.(2);
  expect(source.ready).not.toBe(former);
  const baseline = vi.fn(); const pending = source.ready.then(baseline);
  await Promise.resolve(); expect(baseline).not.toHaveBeenCalled();
  accept(); await pending; expect(admitted).toHaveBeenCalledTimes(2); source.stop();
});

it('preserves HTTP access when the live connection fails without pretending a source was admitted', async () => {
  const status = createStore<SocketStatus>(() => ({ phase: 'connecting', lostAt: null, roundTripMs: null, usualRoundTripMs: null, slow: false, build: null, outdated: false }));
  let accept!: () => void;
  const source = new Promise<void>((resolve) => { accept = resolve; });
  const waiting = admittedOrOffline(source, status);
  status.setState({ phase: 'waiting', lostAt: 1 });
  expect(await waiting).toBe(false);
  status.setState({ phase: 'ready' }); accept();
  expect(await admittedOrOffline(source, status)).toBe(true);
});

it('applies personal events after the baseline and discards held events of a retired reader', () => {
  const values: string[] = []; const release = holdPersonalEvents();
  dispatchPersonalEvent(() => { values.push('newer event'); });
  values.push('baseline'); expect(values).toEqual(['baseline']);
  release(); expect(values).toEqual(['baseline', 'newer event']);
  const former = holdPersonalEvents();
  dispatchPersonalEvent(() => { values.push('former reader'); });
  retirePersonalEvents(); former();
  expect(values).toEqual(['baseline', 'newer event']);
});
