/** The same genuine issuance union works through the existing service-account route. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { createServiceToken } from '@/api/directory';
import type { ApiTokenCreateBody, TokenCreateResponse } from '@/generated/api-types';

const post = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, POST: post } }; });
afterEach(() => post.mockReset());
const service = '00000000-0000-4000-8073-000000000001';
const body: ApiTokenCreateBody = { name: 'service', rules: [{ target: { kind: 'account', id: service, reach: 'self' }, resources: ['key'], allow: [{ operation: 'MeTokenList', actions: ['read'] }] }], proof: { password: 'human proof' }, expires_at: '2030-01-31T12:00:00Z' };
const receipt: TokenCreateResponse = { projection: 'receipt', token_id: '00000000-0000-4000-8074-000000000001', token: 'fixture-service-secret', already_created: false, created_at: '2026-10-01T12:00:00Z', expires_at: body.expires_at ?? null };

it('preserves exact scope/expiry/human proof and the genuine first receipt with one POST', async () => {
  post.mockResolvedValue({ data: receipt, response: new Response(null, { status: 200 }) });
  expect(await createServiceToken(service, body, 'service-act')).toEqual(receipt);
  expect(post).toHaveBeenCalledOnce();
  expect(post).toHaveBeenCalledWith('/service-accounts/{id}/tokens', { params: { path: { id: service } }, body, headers: { 'Idempotency-Key': 'service-act' } });
});

it('keeps a replay secretless instead of retrying the service creation', async () => {
  const replay = { ...receipt, token: null, already_created: true };
  post.mockResolvedValue({ data: replay, response: new Response(null, { status: 200 }) });
  expect(await createServiceToken(service, body, 'service-act')).toEqual(replay);
  expect(post).toHaveBeenCalledOnce();
});
