/** The HTTP baseline follows accepted personal-source admission, including reconnect and reload. */
import { expect, it, vi } from 'vitest';
import { admittedOrOffline, dispatchPersonalEvent, followPersonalSource, holdPersonalEvents, retirePersonalEvents } from '@/realtime/personalSource';
import type { FeedListener, RealtimeSocket, SocketStatus } from '@/realtime/socket';
import { createStore } from 'zustand/vanilla';

function setup() {
  const listeners: FeedListener[] = [];
  const subscribe = vi.fn((_feed: string, listener: FeedListener) => { listeners.push(listener); listener.onSubscribe?.(listeners.length); return { unsubscribe: vi.fn() }; });
  const load = vi.fn(() => Promise.resolve({ seq: 9, complete: true, data: 'baseline' }));
  const admitted = vi.fn(); const apply = vi.fn();
  const source = followPersonalSource({ subscribe } as unknown as RealtimeSocket, { feed: 'user', load, apply, reset: vi.fn(), read: () => 'row', subscribed: admitted }, vi.fn());
  const accept = (listener = listeners.at(-1)!) => listener.onFrame({ op: 'subscribed', feed: 'user', id: 1, seq: 9, mode: 'delta' });
  return { source, listeners, load, apply, admitted, accept };
}

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
  const status = createStore<SocketStatus>(() => ({ phase: 'connecting', lostAt: null, roundTripMs: null, build: null, outdated: false }));
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
