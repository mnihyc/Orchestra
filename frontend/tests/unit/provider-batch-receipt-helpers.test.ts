// @vitest-environment node
/** Batch previews and confirmations acknowledge restricted content through the real client. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ReceiptAcknowledged } from '@/api/client';
import { planProviderUpdates, startProviderUpdates } from '@/api/changes';
import type { ForkCreateBody, ProviderBatchResponse, ProviderRuntimeUpdateBatchPreflightResponse } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => wire.mockReset());

const provider = '00000000-0000-4000-8201-000000000001';
const preflight = '00000000-0000-4000-8201-000000000002';
const batch = '00000000-0000-4000-8201-000000000003';
const body: ForkCreateBody = { preflight_id: preflight, options: { active_turn: 'wait', wait_minutes: 10 } };
const preview: ProviderRuntimeUpdateBatchPreflightResponse = {
  projection: 'receipt', preflight_id: preflight, expires_at: '2030-01-01T00:00:00Z',
  executable: 1, provider_ids: [provider], blocked: [], skipped: [],
};
// An empty jobs list must still honor the explicit discriminator instead of becoming a Full batch.
const accepted: ProviderBatchResponse = { projection: 'receipt', batch_id: batch, jobs: [] };

interface Case {
  name: string;
  status: number;
  path: string;
  body: unknown;
  key: string | null;
  receipt: ProviderRuntimeUpdateBatchPreflightResponse | ProviderBatchResponse;
  send: () => Promise<unknown>;
}
const cases: Case[] = [
  { name: 'preview', status: 200, path: '/api/v1/providers/runtime/updates/preflight',
    body: { provider_ids: [provider] }, key: null, receipt: preview,
    send: () => planProviderUpdates([provider]) },
  { name: 'confirmation', status: 202, path: '/api/v1/providers/runtime/updates?notify=false',
    body, key: 'batch-once', receipt: accepted,
    send: () => startProviderUpdates(body, 'batch-once', false) },
];

it.each(cases)('keeps a $name receipt committed after one exact HTTP write', async (test) => {
  const sent: Request[] = [];
  wire.mockImplementation((request) => {
    sent.push(request.clone());
    return Promise.resolve(new Response(JSON.stringify(test.receipt), {
      status: test.status, headers: { 'Content-Type': 'application/json', 'X-Request-Id': 'batch-accepted-once' },
    }));
  });
  const result: unknown = await test.send().catch((caught: unknown) => caught);
  expect(result).toBeInstanceOf(ReceiptAcknowledged);
  if (!(result instanceof ReceiptAcknowledged)) throw new Error('Expected the accepted batch receipt');
  expect(result.committed).toBe(true);
  expect(result.status).toBe(test.status);
  expect(result.requestId).toBe('batch-accepted-once');
  expect(result.receipt).toEqual(test.receipt);
  expect(wire).toHaveBeenCalledOnce();
  const request = sent[0];
  if (request === undefined) throw new Error('Expected the single accepted request');
  const url = new URL(request.url);
  expect(request.method).toBe('POST');
  expect(url.pathname + url.search).toBe(test.path);
  const actual: unknown = await request.json();
  expect(actual).toEqual(test.body);
  expect(request.headers.get('Idempotency-Key')).toBe(test.key);
});
