/** The status line follows actual first-ready/loss facts, including a new session. */
import type * as Live from '@/realtime/live';
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { RealtimeSocket, SocketLike } from '@/realtime/socket';
import { createRealtimeSocket } from '@/realtime/socket';
import { ConnectionPart, RECONNECT_ELAPSED_AFTER_MS } from '@/features/workbench/StatusParts';
import { useAppearance } from '@/lib/appearance';
import { formatSpan } from '@/lib/time';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

const state = vi.hoisted(() => ({ socket: null as RealtimeSocket | null }));
vi.mock('@/realtime/live', async (original) => {
  const { useStore } = await import('zustand');
  return { ...await original<typeof Live>(), useSocketStatus: () => useStore(state.socket!.status) };
});
class Peer implements SocketLike {
  readyState = 0;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  send() { /* A scripted peer; the component only consumes the real socket's state. */ }
  close(code = 1000) { this.readyState = 3; this.onclose?.({ code }); }
  open() { this.readyState = 1; this.onopen?.(); }
  ready() { this.onmessage?.({ data: JSON.stringify({ op: 'ready', connection_id: 'test', user_id: 'reader', build: 'test-build', heartbeat_secs: 25, server_time: new Date().toISOString(), access_expires_at: null }) }); }
}
let peers: Peer[];
beforeEach(() => {
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-11-07T12:00:00Z'));
  peers = [];
  state.socket = createRealtimeSocket({ token: () => 'test-access', renew: () => Promise.resolve(true), onSuspended: () => undefined,
    url: () => 'ws://localhost/test', random: () => 0.5, connect: () => { const peer = new Peer(); peers.push(peer); return peer; } });
});
afterEach(async () => {
  cleanup(); state.socket?.stop(); state.socket = null; vi.useRealTimers();
  useAppearance.setState({ locale: 'en' }); await setLocale('en');
});
for (const locale of ['en', 'zh-CN'] as const) {
  it(`distinguishes first connection, real loss and a new session in ${locale}`, async () => {
    useAppearance.setState({ locale }); await setLocale(locale);
    state.socket!.start();
    show(<ConnectionPart />);
    expect(screen.getByText(i18n.t('workbench.status.connecting'))).toBeInTheDocument();
    act(() => peers[0]!.close(1006));
    expect(screen.getByText(i18n.t('workbench.status.connecting'))).toBeInTheDocument();
    act(() => state.socket!.wake());
    act(() => { peers.at(-1)!.open(); peers.at(-1)!.ready(); });
    expect(screen.getByText(i18n.t('workbench.status.connected'))).toBeInTheDocument();
    act(() => peers.at(-1)!.close(1006));
    expect(screen.getByText(i18n.t('workbench.status.reconnecting'))).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(RECONNECT_ELAPSED_AFTER_MS - 1_000); });
    expect(screen.getByText(i18n.t('workbench.status.reconnecting'))).toBeInTheDocument();
    act(() => { vi.advanceTimersByTime(1_000); });
    expect(screen.getByText(i18n.t('workbench.status.reconnectingSince', { time: formatSpan(RECONNECT_ELAPSED_AFTER_MS, { locale }, 'short') }))).toBeInTheDocument();
    act(() => { peers.at(-1)!.open(); peers.at(-1)!.ready(); });
    expect(screen.getByText(i18n.t('workbench.status.connected'))).toBeInTheDocument();
    act(() => { state.socket!.stop(); state.socket!.start(); });
    expect(screen.getByText(i18n.t('workbench.status.connecting'))).toBeInTheDocument();
  });
}
it('leaves an idle socket unlabelled', () => {
  show(<div data-testid="connection"><ConnectionPart /></div>);
  expect(screen.getByTestId('connection')).toBeEmptyDOMElement();
});
