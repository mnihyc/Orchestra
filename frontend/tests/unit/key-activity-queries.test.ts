/** Real generated routes keep personal and org activity distinct and preserve opaque cursors. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { tokenActivityQuery } from '@/api/users';
import { orgTokenActivityQuery } from '@/api/directory';
import type { ApiTokenActivityPage } from '@/generated/api-types';

const get = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, GET: get } }; });
const clients: QueryClient[] = [];
afterEach(() => { clients.splice(0).forEach((client) => client.clear()); get.mockReset(); });
const page: ApiTokenActivityPage = { items: [], next_cursor: null };
const client = () => { const value = new QueryClient(); clients.push(value); return value; };

it('reads only the owned personal key path and forwards its opaque cursor and abort signal', async () => {
  get.mockResolvedValue({ data: page, response: new Response(null, { status: 200 }) });
  const query = { limit: 12, cursor: 'opaque|owned|snapshot' };
  expect(await client().query(tokenActivityQuery('owned-key', query))).toEqual(page);
  expect(get).toHaveBeenCalledOnce();
  const anySignal: unknown = expect.any(AbortSignal);
  expect(get).toHaveBeenCalledWith('/me/tokens/{id}/activity', expect.objectContaining({ params: { path: { id: 'owned-key' }, query }, signal: anySignal }));
});

it('uses the real org slice and a separate cache identity for the same key ID', async () => {
  get.mockResolvedValue({ data: page, response: new Response(null, { status: 200 }) });
  const query = { limit: 12, cursor: null };
  await client().query(orgTokenActivityQuery('actual-org', 'owned-key', query));
  expect(get).toHaveBeenCalledWith('/orgs/{id}/tokens/{token_id}/activity', expect.objectContaining({ params: { path: { id: 'actual-org', token_id: 'owned-key' }, query } }));
  expect(orgTokenActivityQuery('actual-org', 'owned-key', query).queryKey).not.toEqual(tokenActivityQuery('owned-key', query).queryKey);
});

it('does not retry a genuine activity refusal or turn it into an empty page', async () => {
  get.mockResolvedValue({ error: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: 'Denied', status: 403, request_id: 'activity-request' }, response: new Response(null, { status: 403 }) });
  await expect(client().query(tokenActivityQuery('owned-key', { limit: 12 }))).rejects.toMatchObject({ status: 403 });
  expect(get).toHaveBeenCalledOnce();
});
