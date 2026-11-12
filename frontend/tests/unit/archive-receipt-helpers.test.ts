// @vitest-environment node
/** Restricted Archive replies use the real common acknowledgement path, once per confirmation. */
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { ReceiptAcknowledged } from '@/api/client';
import { applyArchivePlan } from '@/api/nodes';
import type { NodeArchivePlan, NodeArchiveReceipt } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => wire.mockReset());
const node = '00000000-0000-4000-8301-000000000001';
const preflight = '00000000-0000-4000-8301-000000000002';
const inverse = '00000000-0000-4000-8301-000000000003';
const review = (archiving: boolean): NodeArchivePlan => ({
  node: { id: node, kind: 'topic', name: 'Reviewed topic', redacted: false },
  preflight_id: preflight, stamp: 'review-stamp', revision: 4, expires_at: '2030-01-01T00:00:00Z', archiving,
  subtree: [], counts: { orgs: 0, projects: 0, topics: 1, threads: 0, documents: 0 }, work: [],
  environments: [], copies_kept: 0, live_shares_kept: 0, blockers: [],
});
it.each([true, false])('acknowledges archiving=%s without inventing a Full node or rereading it', async (archiving) => {
  const receipt: NodeArchiveReceipt = { projection: 'receipt', node_id: node, archiving, revision: 5,
    undo: { preflight_id: inverse, stamp: 'inverse-stamp', expected_revision: 5 } };
  const sent: Request[] = [];
  wire.mockImplementation((request) => { sent.push(request.clone()); return Promise.resolve(new Response(JSON.stringify(receipt), {
    status: 200, headers: { 'Content-Type': 'application/json', 'X-Request-Id': 'archive-accepted-once' },
  })); });
  const result: unknown = await applyArchivePlan(review(archiving), 'archive-once').catch((error: unknown) => error);
  expect(result).toBeInstanceOf(ReceiptAcknowledged);
  if (!(result instanceof ReceiptAcknowledged)) throw new Error('Expected the restricted confirmation');
  expect(result.committed).toBe(true); expect(result.receipt).toEqual(receipt);
  expect(result.requestId).toBe('archive-accepted-once');
  expect(wire).toHaveBeenCalledOnce();
  const request = sent[0]; if (request === undefined) throw new Error('Expected the one confirmation');
  expect(request.method).toBe('POST');
  expect(new URL(request.url).pathname).toBe(`/api/v1/nodes/${node}/${archiving ? 'archive' : 'unarchive'}`);
  expect(request.headers.get('Idempotency-Key')).toBe('archive-once');
  const body: unknown = await request.json();
  expect(body).toEqual({ preflight_id: preflight, stamp: 'review-stamp', expected_revision: 4 });
});
