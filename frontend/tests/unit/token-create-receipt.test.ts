/** A dedicated creation consumer preserves the first secret without treating a receipt as failure. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { createToken } from '@/api/users';
import { ApiError } from '@/api/client';
import type { TokenCreateResponse } from '@/generated/api-types';

const post = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  return { ...actual, api: { ...actual.api, POST: post } };
});
afterEach(() => post.mockReset());

const receipt: TokenCreateResponse = { projection: 'receipt', token_id: '00000000-0000-4000-8001-000000000099', token: 'fixture-first-secret', created_at: '2026-10-01T12:00:00Z', expires_at: null, already_created: false };

it('preserves the existing full success reply', async () => {
  const full: TokenCreateResponse = { token: 'fixture-first-secret', already_created: false, info: { id: receipt.token_id, name: 'Scoped work', scopes: [], rules: [], node: null, created_at: receipt.created_at, expires_at: null, last_used_at: null } };
  post.mockResolvedValue({ data: full, response: new Response(null, { status: 201 }) });
  expect(await createToken({ name: 'Scoped work', rules: [] }, 'same-creation')).toEqual(full);
  expect(post).toHaveBeenCalledOnce();
});

it('returns the genuine first-issuance receipt directly, with one request and no reconstructed full DTO', async () => {
  post.mockResolvedValue({ data: receipt, response: new Response(null, { status: 201 }) });
  const reply = await createToken({ name: 'Scoped work', rules: [], proof: { password: 'fixture proof' } }, 'same-creation');
  expect(reply).toEqual(receipt);
  expect(reply).not.toHaveProperty('info');
  expect(post).toHaveBeenCalledOnce();
  expect(post.mock.calls[0]?.[1]).toMatchObject({ body: { rules: [] }, headers: { 'Idempotency-Key': 'same-creation' } });
});

it('preserves the replay receipt without inventing a recoverable secret or retrying', async () => {
  const replay = { ...receipt, token: null, already_created: true };
  post.mockResolvedValue({ data: replay, response: new Response(null, { status: 200 }) });
  expect(await createToken({ name: 'Scoped work', rules: [] }, 'same-creation')).toEqual(replay);
  expect(post).toHaveBeenCalledOnce();
});

it('retains a genuine refusal and never turns it into an accepted result', async () => {
  post.mockResolvedValue({ error: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: 'Denied', status: 403, request_id: 'fixture-request' }, response: new Response(null, { status: 403 }) });
  await expect(createToken({ name: 'Scoped work', rules: [] }, 'same-creation')).rejects.toBeInstanceOf(ApiError);
  expect(post).toHaveBeenCalledOnce();
});
