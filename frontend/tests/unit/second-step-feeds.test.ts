// @vitest-environment node
/** A refused visible feed is asked again when this browser successfully proves its second step, through the ordinary HTTP client. */
import { afterEach, expect, it, vi } from 'vitest';

import type { AuthResult, ClientFrame, Me, ServerFrame } from '@/generated/api-types';
import type { SocketLike } from '@/realtime/socket';

class ServerSocket implements SocketLike {
  static latest: ServerSocket | undefined;
  readyState = 1;
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  sent: ClientFrame[] = [];

  constructor() { ServerSocket.latest = this; }
  send(data: string): void { this.sent.push(JSON.parse(data) as ClientFrame); }
  close(): void { this.readyState = 3; }
  say(frame: ServerFrame): void { this.onmessage?.({ data: JSON.stringify(frame) }); }
  subscriptions() { return this.sent.filter((frame): frame is Extract<ClientFrame, { op: 'sub' }> => frame.op === 'sub'); }
}

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});

it('keeps a refused feed closed for renewal or a wrong code, and resubscribes after verification or enabling the authenticator', async () => {
  vi.useFakeTimers();
  vi.stubGlobal('location', { origin: 'http://orch.test', protocol: 'http:', host: 'orch.test' });
  vi.stubGlobal('WebSocket', ServerSocket);
  let proved = false;
  const me = { id: 'u1', username: 'ada', display_name: 'Ada' } as Me;
  vi.stubGlobal('fetch', (request: Request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith('/me/second-step/verify') && !proved) return Promise.resolve(new Response(JSON.stringify({ type: 'orch:error:request.invalid', title: 'Invalid code', status: 400, code: 'request.invalid', detail: 'The code is invalid.', request_id: 'r1' }), { status: 400, headers: { 'Content-Type': 'application/problem+json' } }));
    const answer = path.endsWith('/me/totp/enable') ? { me, recovery_codes: { codes: [], status: { remaining: 0, created_at: null, methods: ['totp'], recent_recovery: false } } } : me;
    return Promise.resolve(new Response(JSON.stringify(answer), { status: 200, headers: { 'Content-Type': 'application/json' } }));
  });
  const { client, ApiError } = await import('@/api/client');
  const { socket } = await import('@/realtime/live');
  const { enableAuthenticator, verifySecondStep } = await import('@/api/users');
  const grant = (token: string): AuthResult => ({ access_token: token, access_expires_at: new Date(Date.now() + 900_000).toISOString(), user: me });
  client.adopt(grant('token-1'));
  const wire = ServerSocket.latest;
  if (wire === undefined) throw new Error('signing in must open the live connection');
  wire.onopen?.();
  wire.say({ op: 'ready', connection_id: 'c1', user_id: me.id, server_time: '', access_expires_at: '', heartbeat_secs: 25, build: 'b1' });
  const subscription = socket.subscribe('org:held', { since: () => null, onFrame: vi.fn() });
  const first = wire.subscriptions()[0];
  if (first === undefined) throw new Error('the visible feed must be requested');
  wire.say({ op: 'error', id: first.id, code: 'forbidden', detail: 'This session cannot follow the feed.' });
  client.adopt(grant('token-2'));
  await expect(verifySecondStep({ totp: '000000' })).rejects.toBeInstanceOf(ApiError);
  expect(wire.subscriptions()).toHaveLength(1);
  proved = true;
  await verifySecondStep({ totp: '123456' });
  expect(wire.subscriptions().map((frame) => frame.feed)).toEqual(['org:held', 'org:held']);
  const retried = wire.subscriptions().at(-1);
  if (retried === undefined) throw new Error('successful proof must retry the held feed');
  expect(retried.id).toBeGreaterThan(first.id);
  wire.say({ op: 'error', id: retried.id, code: 'forbidden', detail: 'This session cannot follow the feed.' });
  await enableAuthenticator('123456');
  expect(wire.subscriptions()).toHaveLength(3);
  subscription.unsubscribe();
  socket.stop();
  await client.signOut();
});
