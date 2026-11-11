// @vitest-environment node
/** Accepted request identities reach their forms through the real shared API client. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ApiError } from '@/api/client';
import { requestAccess, requestJoin } from '@/api/access';
import type { AccessRequestReceipt } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => wire.mockReset());

const receipt: AccessRequestReceipt = { projection: 'receipt', batch_id: 'accepted-batch', approval_ids: ['accepted-approval'] };
const requests = [
  { name: 'access', path: '/api/v1/access-requests', send: () => requestAccess({ items: [{ object: { kind: 'thread', id: 'target' }, permissions: ['thread.view'] }], reason: 'Please review' }, 'ask-once') },
  { name: 'join', path: '/api/v1/chats/group/join-requests', send: () => requestJoin('group', { reason: 'Please review' }, 'ask-once') },
] as const;

it.each(requests)('$name returns the real accepted receipt without another read or send', async ({ path, send }) => {
  wire.mockResolvedValueOnce(new Response(JSON.stringify(receipt), { status: 200, headers: { 'Content-Type': 'application/json' } }));
  expect(await send()).toEqual(receipt);
  expect(wire).toHaveBeenCalledOnce();
  const request = wire.mock.calls[0]?.[0];
  expect(new URL(request?.url ?? '').pathname).toBe(path);
  expect(request?.headers.get('Idempotency-Key')).toBe('ask-once');
});

it.each(requests)('$name preserves a refused request as an error', async ({ send }) => {
  wire.mockResolvedValueOnce(new Response(JSON.stringify({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Access denied', detail: 'The request is outside this credential.', request_id: 'denied-request' }), { status: 403, headers: { 'Content-Type': 'application/json' } }));
  await expect(send()).rejects.toBeInstanceOf(ApiError);
  expect(wire).toHaveBeenCalledOnce();
});
