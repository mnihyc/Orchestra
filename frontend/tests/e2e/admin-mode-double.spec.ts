/** Actual fake-server HTTP routes and login suppliers, never page.route fulfillment. */
import { expect, test, type APIRequestContext } from '@playwright/test';
import type { AdminMode, AuthResult, MeBootstrapReply } from '../../src/generated/api-types.ts';
import { WebSocket } from 'ws';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const api = `${DOUBLE}/api/v1`;

async function signed(request: APIRequestContext, login = 'ada', password = 'correct horse battery') {
  const response = await request.post(`${api}/auth/login`, { data: { login, password } });
  expect(response.status()).toBe(200);
  const auth = await response.json() as AuthResult;
  return { Authorization: `Bearer ${auth.access_token}` };
}

test.beforeEach(async ({ request }) => { expect((await request.post(`${DOUBLE}/__double/reset`, { data: {} })).status()).toBe(204); });

test('the shared server implements the native GET PUT bootstrap contract independently for two actual logins', async ({ request }) => {
  const a = await signed(request), b = await signed(request);
  const mode = async (headers: typeof a): Promise<AdminMode> => {
    const response = await request.get(`${api}/me/admin-mode`, { headers });
    expect(response.status()).toBe(200);
    return response.json() as Promise<AdminMode>;
  };
  expect(await mode(a)).toEqual({ as_admin: false, available: true });
  const enabled = await request.put(`${api}/me/admin-mode`, { headers: a, data: { as_admin: true } });
  expect(enabled.status()).toBe(200);
  expect(await enabled.json()).toEqual({ as_admin: true, available: true });
  expect(await mode(b)).toEqual({ as_admin: false, available: true });
  for (const [headers, as_admin] of [[a, true], [b, false]] as const) {
    const response = await request.get(`${api}/me/bootstrap`, { headers });
    expect(response.status()).toBe(200);
    const opening = await response.json() as MeBootstrapReply;
    expect(opening.admin_mode).toEqual({ as_admin, available: true });
    expect(opening.person.is_system_admin).toBe(true);
  }
});

test('the shared route refuses non-admin and invalid enablement instead of returning fake success', async ({ request }) => {
  const ordinary = await signed(request, 'dora', 'compilers and goto');
  const response = await request.put(`${api}/me/admin-mode`, { headers: ordinary, data: { as_admin: true } });
  expect(response.status()).toBe(403);
  expect(await response.json()).toMatchObject({ code: 'authz.denied', status: 403 });
  const stillOff = await request.get(`${api}/me/admin-mode`, { headers: ordinary });
  expect(await stillOff.json()).toEqual({ as_admin: false, available: false });
  const admin = await signed(request);
  const invalid = await request.put(`${api}/me/admin-mode`, { headers: admin, data: { as_admin: true, session_id: 'another browser' } });
  expect(invalid.status()).toBe(400);
  expect(await invalid.json()).toMatchObject({ code: 'request.invalid', status: 400 });
  expect(await (await request.get(`${api}/me/admin-mode`, { headers: admin })).json()).toEqual({ as_admin: false, available: true });
});

async function wire(headers: { Authorization: string }) {
  const socket = new WebSocket(`${DOUBLE.replace('http:', 'ws:')}/api/v1/ws`, 'orch.v1');
  const rows: Record<string, unknown>[] = [], seen: Record<string, unknown>[] = [];
  const waiting = new Set<() => void>();
  let failure: Error | null = null;
  socket.on('message', (bytes) => {
    const buffer = Array.isArray(bytes) ? Buffer.concat(bytes) : Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes);
    const frame = JSON.parse(buffer.toString('utf8')) as Record<string, unknown>;
    rows.push(frame); seen.push(frame);
    for (const wake of [...waiting]) wake();
  });
  socket.on('error', (error) => { failure = error; for (const wake of [...waiting]) wake(); });
  socket.on('close', () => { failure = new Error('The owned socket closed before its witness'); for (const wake of [...waiting]) wake(); });
  const next = (op: string) => new Promise<Record<string, unknown>>((resolve, reject) => {
    const take = () => {
      const index = rows.findIndex((frame) => frame.op === op);
      if (index >= 0) { waiting.delete(take); resolve(rows.splice(index, 1)[0]!); }
      else if (failure !== null) { waiting.delete(take); reject(failure); }
    };
    waiting.add(take); take();
  });
  try {
    await new Promise<void>((resolve, reject) => { socket.once('open', resolve); socket.once('error', reject); });
    socket.send(JSON.stringify({ op: 'auth', token: headers.Authorization.slice('Bearer '.length) }));
    await next('ready');
    socket.send(JSON.stringify({ op: 'sub', id: 1, feed: 'user' }));
    await next('subscribed');
  } catch (error) { socket.close(); throw error; }
  return { socket, next, seen };
}

test('a mode change retires only that session’s live subscriptions while its peer keeps the same connection', async ({ request }) => {
  const a = await signed(request), b = await signed(request);
  const first = await wire(a);
  let peer: Awaited<ReturnType<typeof wire>> | undefined;
  try {
    peer = await wire(b);
    const enabled = await request.put(`${api}/me/admin-mode`, { headers: a, data: { as_admin: true } });
    expect(enabled.status()).toBe(200);
    const change = await first.next('event');
    expect(change).toMatchObject({ id: 1, feed: 'user', data: { what: 'admin_mode', as_admin: true } });
    expect(await first.next('resync')).toEqual({ op: 'resync', id: 1, feed: 'user' });
    const sessions = await request.get(`${api}/me/sessions`, { headers: a });
    const rows = await sessions.json() as { id: string; current: boolean }[];
    expect((change.data as { session_id: string }).session_id).toBe(rows.find((row) => row.current)?.id);
    peer.socket.send(JSON.stringify({ op: 'ping', n: 41 }));
    expect(await peer.next('pong')).toEqual({ op: 'pong', n: 41 });
    expect(peer.seen.filter((frame) => frame.op === 'resync')).toEqual([]);
    first.socket.send(JSON.stringify({ op: 'sub', id: 1, feed: 'user' }));
    expect(await first.next('subscribed')).toMatchObject({ id: 1, feed: 'user' });
    const stillOff = await request.get(`${api}/me/admin-mode`, { headers: b });
    expect(await stillOff.json()).toEqual({ as_admin: false, available: true });
  } finally { first.socket.close(); peer?.socket.close(); }
});
