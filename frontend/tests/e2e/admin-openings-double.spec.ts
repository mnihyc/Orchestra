/** Actual HTTP/WS producers and the unchanged audit route; no audit-row injection. */
import { randomUUID } from 'node:crypto';
import { expect, test, type APIRequestContext } from '@playwright/test';
import { WebSocket } from 'ws';
import type { AuditEntry, AuditEntryPage, AuthResult, SessionInfo } from '../../src/generated/api-types.ts';
import { ACME } from '../doubles/fixtures.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const api = `${DOUBLE}/api/v1`;
async function signed(request: APIRequestContext) {
  const response = await request.post(`${api}/auth/login`, { data: { login: 'ada', password: 'correct horse battery' } });
  expect(response.status()).toBe(200);
  const auth = await response.json() as AuthResult;
  const headers = { Authorization: `Bearer ${auth.access_token}` };
  expect((await request.put(`${api}/me/admin-mode`, { headers, data: { as_admin: true } })).status()).toBe(200);
  return headers;
}
async function make(request: APIRequestContext, user = 'dora') {
  const response = await request.post(`${DOUBLE}/__double/documents/make`, { data: { user, node: ACME.fixLogin, title: `Actual private opening ${randomUUID()}`, private: true, count: 1 } });
  expect(response.status()).toBe(200);
  return (await response.json() as { id: string }).id;
}
async function audit(request: APIRequestContext, headers: { Authorization: string }): Promise<AuditEntry[]> {
  const response = await request.get(`${api}/admin/audit?action=admin.opened&limit=200`, { headers });
  expect(response.status()).toBe(200);
  return (await response.json() as AuditEntryPage).items;
}
async function admitted(headers: { Authorization: string }, id: string, nonce: string) {
  const socket = new WebSocket(`${DOUBLE.replace('http:', 'ws:')}/api/v1/ws`, 'orch.v1');
  const rows: Record<string, unknown>[] = [], waiting = new Set<() => void>();
  let failure: Error | null = null;
  socket.on('message', (bytes) => {
    const buffer = Array.isArray(bytes) ? Buffer.concat(bytes) : Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes);
    rows.push(JSON.parse(buffer.toString('utf8')) as Record<string, unknown>);
    for (const wake of [...waiting]) wake();
  });
  socket.on('error', (error) => { failure = error; for (const wake of [...waiting]) wake(); });
  socket.on('close', () => { failure = new Error('The owned document socket closed before its admission'); for (const wake of [...waiting]) wake(); });
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
    socket.send(JSON.stringify({ op: 'sub', id: 9, feed: `doc:${id}`, opening_id: nonce }));
    const reply = await next('subscribed');
    expect(reply).toMatchObject({ id: 9, feed: `doc:${id}` });
    return socket;
  } catch (error) { socket.close(); throw error; }
}

test.beforeEach(async ({ request }) => { expect((await request.post(`${DOUBLE}/__double/reset`, { data: {} })).status()).toBe(204); });

test('real private HTTP, cached and admitted live readers share a native audit receipt while sessions and openings remain distinct', async ({ request }) => {
  const headers = await signed(request), id = await make(request), nonce = randomUUID();
  const read = await request.get(`${api}/documents/${id}`, { headers: { ...headers, 'x-orch-opening': nonce } });
  expect(read.status()).toBe(200);
  const original = await audit(request, headers);
  expect(original).toHaveLength(1);
  const sessions = await request.get(`${api}/me/sessions`, { headers });
  const current = (await sessions.json() as SessionInfo[]).find((row) => row.current)!;
  expect(original[0]!.details).toEqual({ place: 'system', session_id: current.id, opening_id: nonce });
  expect(original[0]).toMatchObject({ action: 'admin.opened', as_admin: true, history: false, override: false,
    node: null, object: { kind: 'document', id }, summary: 'opened a thing through administrative access', request_id: read.headers()['x-request-id'] });
  const body = { kind: 'document', id, opening_id: nonce };
  const cached = await request.post(`${api}/me/admin-openings`, { headers: { ...headers, 'x-orch-opening': randomUUID() }, data: body });
  expect(cached.status()).toBe(204);
  const socket = await admitted(headers, id, nonce);
  try {
    expect(await audit(request, headers)).toEqual(original);
    const peer = await signed(request);
    expect((await request.post(`${api}/me/admin-openings`, { headers: peer, data: body })).status()).toBe(204);
    expect((await request.post(`${api}/me/admin-openings`, { headers, data: { ...body, opening_id: randomUUID() } })).status()).toBe(204);
    const rows = await audit(request, headers);
    expect(rows).toHaveLength(3);
    expect(rows.find((row) => row.id === original[0]!.id)).toEqual(original[0]);
    const local = await request.get(`${api}/nodes/${ACME.fixLogin}/audit?action=admin.opened`, { headers });
    expect(local.status()).toBe(200);
    expect((await local.json() as AuditEntryPage).items).toEqual([]);
  } finally { socket.close(); }
});

test('denied ordinary mode-off and malformed requests create no private-opening rows', async ({ request }) => {
  const headers = await signed(request), id = await make(request), own = await make(request, 'ada'), nonce = randomUUID();
  const ordinary = await request.get(`${api}/documents/${own}`, { headers: { ...headers, 'x-orch-opening': nonce } });
  expect(ordinary.status()).toBe(200);
  expect((await request.post(`${api}/me/admin-openings`, { headers, data: { kind: 'document', id: own, opening_id: nonce } })).status()).toBe(204);
  const malformed = await request.post(`${api}/me/admin-openings`, { headers, data: { kind: 'document', id, opening_id: 'not-a-UUID' } });
  expect(malformed.status()).toBe(400);
  expect(await malformed.json()).toMatchObject({ code: 'request.invalid' });
  const badHeader = await request.get(`${api}/documents/${id}`, { headers: { ...headers, 'x-orch-opening': 'not-a-UUID' } });
  expect(badHeader.status()).toBe(400);
  expect((await request.put(`${api}/me/admin-mode`, { headers, data: { as_admin: false } })).status()).toBe(200);
  expect((await request.get(`${api}/documents/${id}`, { headers })).status()).toBe(404);
  expect((await request.post(`${api}/me/admin-openings`, { headers, data: { kind: 'document', id, opening_id: nonce } })).status()).toBe(204);
  expect((await request.put(`${api}/me/admin-mode`, { headers, data: { as_admin: true } })).status()).toBe(200);
  expect(await audit(request, headers)).toEqual([]);
});
