// @vitest-environment node
/** Native summary receipts stay acknowledged; source text is always a separate current read. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { refreshResourceSummary, resourceSummaryQuery, setDocumentSummaryOptions } from '@/api/summaries';
import type { SummaryOptionsReceipt, SummaryReceipt, SummarySnapshot } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => wire.mockReset());

const document = '00000000-0000-4000-8202-000000000001';
const chat = '00000000-0000-4000-8202-000000000002';
const nonce = '00000000-0000-4000-8202-000000000003';
const root = '00000000-0000-4000-8202-000000000004';
function reply(body: SummaryOptionsReceipt | SummaryReceipt | SummarySnapshot, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
}

it('accepts one document-options receipt without treating the committed write as a failure or reading content', async () => {
  const receipt: SummaryOptionsReceipt = { projection: 'receipt', target: { kind: 'document', id: document }, revision: 3 };
  const sent: Request[] = [];
  wire.mockImplementation((request) => { sent.push(request.clone()); return Promise.resolve(reply(receipt)); });
  await expect(setDocumentSummaryOptions(document, null, 2)).resolves.toEqual(receipt);
  expect(wire).toHaveBeenCalledOnce();
  const request = sent[0];
  if (request === undefined) throw new Error('The single native options write is required');
  expect(request.method).toBe('PUT');
  expect(new URL(request.url).pathname).toBe(`/api/v1/documents/${document}/summary-options`);
  expect(await request.json()).toEqual({ options: null, expected_revision: 2 });
});

it('acknowledges the native refresh receipt once with its original nonce and pinned chat range', async () => {
  const target = { kind: 'chat' as const, id: chat };
  const receipt: SummaryReceipt = { projection: 'receipt', id: nonce, target, revision: 4 };
  const sent: Request[] = [];
  wire.mockImplementation((request) => { sent.push(request.clone()); return Promise.resolve(reply(receipt, 202)); });
  await expect(refreshResourceSummary(target, nonce, { after: 0, through: 7 })).resolves.toEqual(receipt);
  expect(wire).toHaveBeenCalledOnce();
  const request = sent[0];
  if (request === undefined) throw new Error('The single native refresh is required');
  expect(request.method).toBe('POST');
  expect(new URL(request.url).pathname).toBe(`/api/v1/summaries/chat/${chat}/refresh`);
  expect(await request.json()).toEqual({ client_id: nonce, range: { after: 0, through: 7 } });
});

it.each([undefined, { after: 0, through: 7 }])('serializes the generated nullable range without inventing cursor values (%j)', async (range) => {
  const target = { kind: 'chat' as const, id: chat };
  const snapshot: SummarySnapshot = { target, root_org_id: root, inspecting: false, revision: 0,
    state: 'empty', text: null, producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null };
  const sent: Request[] = [];
  wire.mockImplementation((request) => { sent.push(request.clone()); return Promise.resolve(reply(snapshot)); });
  const cache = new QueryClient();
  try { await expect(cache.fetchQuery(resourceSummaryQuery(target, range))).resolves.toEqual(snapshot); }
  finally { cache.clear(); }
  expect(wire).toHaveBeenCalledOnce();
  const request = sent[0];
  if (request === undefined) throw new Error('The native free read is required');
  const url = new URL(request.url);
  expect(request.method).toBe('GET');
  expect(url.search).toBe(range === undefined ? '' : '?after=0&through=7');
});
