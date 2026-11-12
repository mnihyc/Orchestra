/** Native HTTP/ready timestamps keep the queued countdown off a device's incorrect wall clock. */
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createApiClient } from '@/api/client';
import { i18n } from '@/i18n';
import { serverClock } from '@/lib/serverClock';
import { formatSpan } from '@/lib/time';
import { createRealtimeSocket, type RealtimeSocket, type SocketLike } from '@/realtime/socket';
import { ThreadStateChip } from '@/ui/ThreadState';
import { display, recoveryDisplay } from '../doubles/thread-display';
import { meta } from '../doubles/fixtures';
import { show } from './support';

const serverAt = Date.parse('2026-11-07T12:00:00Z');
class Peer implements SocketLike {
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  send() { /* The real socket owns authentication and clock receipt. */ }
  close() { this.readyState = 3; }
  ready(at: string) {
    this.readyState = 1; this.onopen?.();
    this.onmessage?.({ data: JSON.stringify({ op: 'ready', connection_id: 'test', user_id: 'reader', build: 'test',
      heartbeat_secs: 25, server_time: at, access_expires_at: null }) });
  }
}
let socket: RealtimeSocket | undefined;
beforeEach(() => { vi.useFakeTimers(); serverClock.clear(); });
afterEach(() => { cleanup(); socket?.stop(); socket = undefined; serverClock.clear(); vi.clearAllTimers(); vi.useRealTimers(); });

for (const skew of [-300_000, 300_000]) {
  it(`uses answered HTTP and realtime clock samples with device offset ${skew}`, async () => {
    vi.setSystemTime(serverAt + skew);
    const queued = display('queued', { recovery: recoveryDisplay({ attempts: null, due_at: new Date(serverAt + 30_000).toISOString() }) });
    show(<ThreadStateChip display={queued} />);
    expect(screen.getByText(i18n.t('state.queued'))).toBeVisible();
    const transport = createApiClient({ baseUrl: 'http://localhost/api/v1', clock: serverClock,
      fetch: async () => {
        await new Promise((resolve) => setTimeout(resolve, 2_000));
        // The sample lies at the middle of this deliberately delayed native round trip.
        return Response.json(meta(), { headers: { Date: new Date(serverAt + 1_000).toUTCString() } });
      } });
    const answer = transport.api.GET('/meta');
    await act(async () => { await vi.advanceTimersByTimeAsync(2_000); await answer; });
    const remaining = (milliseconds: number) => i18n.t('threadStates.retryIn', { time: formatSpan(milliseconds, { locale: 'en' }, 'short') });
    expect(screen.getByText(remaining(28_000))).toBeVisible();
    void act(() => vi.advanceTimersByTime(1_000));
    expect(screen.getByText(remaining(27_000))).toBeVisible();
    const peer = new Peer();
    socket = createRealtimeSocket({ token: () => 'test-access', renew: () => Promise.resolve(true), onSuspended: () => undefined,
      url: () => 'ws://localhost/test', connect: () => peer, clock: serverClock });
    act(() => { socket!.start(); peer.ready(new Date(serverAt + 10_000).toISOString()); });
    expect(screen.getByText(remaining(20_000))).toBeVisible();
    act(() => serverClock.observe('not a timestamp', Date.now(), Date.now()));
    expect(screen.getByText(remaining(20_000))).toBeVisible();
    void act(() => vi.advanceTimersByTime(30_000));
    expect(screen.getByText(remaining(0))).toBeVisible();
  });
}
