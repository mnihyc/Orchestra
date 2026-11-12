// @vitest-environment node
/** Native summary receipts stay acknowledged; source text is always a separate current read. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { refreshResourceSummary, resourceSummaryQuery, setDocumentSummaryOptions } from '@/api/summaries';
import type { SummaryOptionsReceipt, SummaryReceipt, SummarySnapshot, SummaryTarget, SummaryChatRange } from '@/generated/api-types';
import { readSummarySnapshot } from '@/api/summarySnapshots';
import { summaryReaderKey } from '@/api/summaryReader';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => { wire.mockReset(); vi.unstubAllGlobals(); });

const document = '00000000-0000-4000-8202-000000000001';
const chat = '00000000-0000-4000-8202-000000000002';
const nonce = '00000000-0000-4000-8202-000000000003';
const root = '00000000-0000-4000-8202-000000000004';
function reply(body: unknown, status = 200): Response {
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

it.each([undefined, { after: 0, through: 7 }])('batches the pinned native range without inventing cursor values (%j)', async (range) => {
  const target = { kind: 'chat' as const, id: chat };
  const snapshot: SummarySnapshot = { target, root_org_id: root, inspecting: false, revision: 0,
    state: 'empty', text: null, producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null };
  const sent: Request[] = [];
  wire.mockImplementation((request) => { sent.push(request.clone()); return Promise.resolve(reply({ items: [{ ...snapshot, full: snapshot }], unavailable: [], next_offset: null })); });
  const cache = new QueryClient();
  try { await expect(cache.query(resourceSummaryQuery(target, range))).resolves.toEqual(snapshot); }
  finally { cache.clear(); }
  expect(wire).toHaveBeenCalledOnce();
  const request = sent[0];
  if (request === undefined) throw new Error('The native free read is required');
  const url = new URL(request.url);
  expect(request.method).toBe('POST');
  expect(url.pathname).toBe('/api/v1/summaries/batch');
  expect(await request.json()).toEqual({ targets: [target], offset: 0, full: true, ranges: range === undefined ? {} : { [chat]: range } });
});


it('joins the cold thread and its opening-dependent chat views in one current native read', async () => {
  const frames: FrameRequestCallback[] = [];
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => { frames.push(callback); return frames.length; });
  const thread = { kind: 'thread', id: document } as const;
  const catchUp = { kind: 'chat', id: chat } as const;
  const title = { kind: 'chat_title', id: chat } as const;
  let opened!: () => void;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const sent: Request[] = [];
  wire.mockImplementation(async (request) => {
    sent.push(request.clone());
    const body = await request.json() as { targets: SummaryTarget[]; ranges: Record<string, SummaryChatRange>; full: boolean };
    return reply({ items: body.targets.map((target) => {
      const full: SummarySnapshot = { target, root_org_id: root, inspecting: false, revision: 1, state: 'fresh', text: target.kind,
        producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null };
      return { ...full, full };
    }), unavailable: [], next_offset: null });
  });
  try {
    const first = cache.query(resourceSummaryQuery(thread));
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(wire).not.toHaveBeenCalled();
    frames.shift()?.(0);
    await Promise.resolve();
    // The chat opening mounts after the thread's first paint, just as the actual sibling panel does.
    const opening = cache.query({ queryKey: ['node-chat-opening', document], queryFn: () => new Promise<boolean>((resolve) => { opened = () => resolve(true); }) });
    frames.shift()?.(16);
    await Promise.resolve();
    opened(); await opening;
    const reads = [first, cache.query(resourceSummaryQuery(catchUp, { after: 0, through: 7 })), cache.query(resourceSummaryQuery(title))];
    await Promise.resolve(); await Promise.resolve();
    await vi.waitFor(() => { expect(frames.length).toBeGreaterThan(0); });
    frames.shift()?.(32);
    const values = await Promise.all(reads);
    expect(values.map((value) => value.text)).toEqual(['thread', 'chat', 'chat_title']);
    expect(sent).toHaveLength(1);
    expect(await sent[0]?.json()).toEqual({ targets: [thread, catchUp, title], offset: 0, full: true, ranges: { [chat]: { after: 0, through: 7 } } });
  } finally { cache.clear(); }
});

it('keeps concurrent captured ranges for the same chat in distinct native reads', async () => {
  const target = { kind: 'chat', id: chat } as const;
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  wire.mockImplementation(async (request) => {
    const body = await request.json() as { ranges: Record<string, SummaryChatRange> };
    const full: SummarySnapshot = { target, root_org_id: root, inspecting: false, revision: 1, state: 'fresh', text: JSON.stringify(body.ranges[chat]),
      producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null };
    return reply({ items: [{ ...full, full }], unavailable: [], next_offset: null });
  });
  try {
    const ranges = [{ after: 0, through: 7 }, { after: 7, through: 14 }];
    const result = await Promise.all(ranges.map((range) => cache.query(resourceSummaryQuery(target, range))));
    expect(result.map((value) => value.text)).toEqual(ranges.map((range) => JSON.stringify(range)));
    expect(wire).toHaveBeenCalledTimes(2);
  } finally { cache.clear(); }
});

it('cancels one shared read without retiring another source or sending a paid act', async () => {
  const cache = new QueryClient();
  const firstTarget = { kind: 'document', id: document } as const, secondTarget = { kind: 'chat_title', id: chat } as const;
  const firstController = new AbortController(), secondController = new AbortController();
  let deliver!: () => void;
  let requestSignal: AbortSignal | undefined;
  wire.mockImplementation((request) => {
    requestSignal = request.signal;
    return new Promise<Response>((resolve) => { deliver = () => resolve(reply({
      items: [firstTarget, secondTarget].map((target) => {
        const full: SummarySnapshot = { target, root_org_id: root, inspecting: false, revision: 1, state: 'fresh', text: target.kind,
          producer: null, updated_at: null, route: null, input: null, manual_problem: null, wait: null, poll_after_ms: null, problem: null };
        return { ...full, full };
      }), unavailable: [], next_offset: null,
    })); });
  });
  const first = readSummarySnapshot(cache, firstTarget, undefined, summaryReaderKey(), firstController.signal).catch((error: unknown) => error);
  const second = readSummarySnapshot(cache, secondTarget, undefined, summaryReaderKey(), secondController.signal);
  try {
    await vi.waitFor(() => { expect(wire).toHaveBeenCalledOnce(); });
    firstController.abort();
    expect(await first).toMatchObject({ name: 'AbortError' });
    expect(requestSignal?.aborted).toBe(false);
    deliver();
    expect((await second).target).toEqual(secondTarget);
    expect(wire).toHaveBeenCalledOnce();
  } finally { firstController.abort(); secondController.abort(); cache.clear(); }
});
