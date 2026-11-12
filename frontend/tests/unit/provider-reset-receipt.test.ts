/** Successful reset receipts are real outcomes, without invented provider content or automatic resend. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { consumeReset } from '@/api/provider-plan';
import type { ProviderResetConsumeBody, ProviderResetConsumeResponse } from '@/generated/api-types';
import { planProvider } from './provider-plan-fixtures';

const post = vi.hoisted(() => vi.fn());
vi.mock('@/api/client', async (load) => { const actual = await load<typeof Client>(); return { ...actual, api: { ...actual.api, POST: post } }; });
afterEach(() => post.mockReset());
const body: ProviderResetConsumeBody = { request_id: '00000000-0000-4000-8001-000000000001', reset_id: 'reset-credit', available_count: 1 };

it('returns the restricted receipt outcome once without reconstructing provider facts', async () => {
  const receipt: ProviderResetConsumeResponse = { projection: 'receipt', provider_id: '00000000-0000-4000-8001-000000000002', request_id: body.request_id, outcome: 'nothingToReset' };
  post.mockResolvedValue({ data: receipt, response: new Response(null, { status: 200 }) });
  const result = await consumeReset(receipt.provider_id, body);
  expect(result).toEqual(receipt); expect(result).not.toHaveProperty('provider');
  expect(post).toHaveBeenCalledOnce();
  expect(post).toHaveBeenCalledWith('/providers/{id}/resets/consume', { params: { path: { id: receipt.provider_id } }, body });
});

it('preserves the existing full outcome and provider projection', async () => {
  const provider = planProvider();
  const full: ProviderResetConsumeResponse = { outcome: 'reset', provider };
  post.mockResolvedValue({ data: full, response: new Response(null, { status: 200 }) });
  expect(await consumeReset(provider.id, body)).toEqual(full); expect(post).toHaveBeenCalledOnce();
});

it('keeps a genuine refusal rejected and sends no extra reset', async () => {
  post.mockResolvedValue({ error: { type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Denied', detail: 'Denied', status: 403, request_id: 'reset-request' }, response: new Response(null, { status: 403 }) });
  await expect(consumeReset('provider', body)).rejects.toMatchObject({ status: 403 });
  expect(post).toHaveBeenCalledOnce();
});
