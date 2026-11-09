// @vitest-environment node
/** Actual native-client batch transport, not a synthetic summary endpoint or authority proof. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { searchSummaryProjectionQuery } from '@/api/summaries';
import type { SummaryBatchBody, SummaryBatchReply, SummaryProjection, SummaryTarget } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
const before = client.session.getState();
afterEach(() => { wire.mockReset(); client.session.setState(before, true); });
const target = (index: number): SummaryTarget => ({ kind: 'document', id: `00000000-0000-4000-8202-${index.toString().padStart(12, '0')}` });
const projection = (target: SummaryTarget): SummaryProjection => ({ target, root_org_id: 'root', inspecting: false, text: `Stored ${target.id}`, producer: null, updated_at: null });
const response = (body: SummaryBatchReply) => new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });

it('coalesces twenty distinct current targets into one native read and makes no per-target GET or refresh', async () => {
  const sent: Request[] = [];
  wire.mockImplementation(async (request) => {
    sent.push(request);
    const body = await request.clone().json() as SummaryBatchBody;
    return response({ items: body.targets.map(projection), unavailable: [], next_offset: null });
  });
  const cache = new QueryClient();
  try {
    const targets = Array.from({ length: 20 }, (_, index) => target(index + 1));
    const rows = await Promise.all(targets.map((target) => cache.query(searchSummaryProjectionQuery(target))));
    expect(rows.map((row) => row.target)).toEqual(targets);
    expect(sent).toHaveLength(1); expect(sent[0]?.method).toBe('POST');
    expect(sent[0]?.url).toBe('http://orch.test/api/v1/summaries/batch');
    expect(await sent[0]?.clone().json()).toEqual({ targets, offset: 0 });
  } finally { cache.clear(); }
});

it('continues bounded native windows and rejects only the unavailable source', async () => {
  const offsets: number[] = [];
  wire.mockImplementation(async (request) => {
    const body = await request.json() as SummaryBatchBody; const offset = body.offset; offsets.push(offset);
    const selected = body.targets.slice(offset, offset + 2);
    const unavailable = selected.filter((one) => one.id === target(3).id);
    return response({ items: selected.filter((one) => one.id !== target(3).id).map(projection), unavailable,
      next_offset: offset + 2 < body.targets.length ? offset + 2 : null });
  });
  const cache = new QueryClient();
  try {
    const result = await Promise.allSettled(Array.from({ length: 5 }, (_, index) => cache.query(searchSummaryProjectionQuery(target(index + 1)))));
    expect(offsets).toEqual([0, 2, 4]);
    expect(result.map((row) => row.status)).toEqual(['fulfilled', 'fulfilled', 'rejected', 'fulfilled', 'fulfilled']);
  } finally { cache.clear(); }
});

it('retires an old reader batch before its body can enter a replacement reader cache', async () => {
  let release: (() => void) | undefined;
  wire.mockImplementation(async (request) => {
    const body = await request.json() as SummaryBatchBody;
    await new Promise<void>((resolve) => { release = resolve; });
    return response({ items: body.targets.map(projection), unavailable: [], next_offset: null });
  });
  const cache = new QueryClient();
  try {
    const pending = cache.query(searchSummaryProjectionQuery(target(1)));
    const result = pending.catch((error: unknown) => error);
    await vi.waitFor(() => expect(release).toEqual(expect.any(Function)));
    client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 });
    release?.();
    expect(await result).toBeInstanceOf(DOMException);
    expect(cache.getQueryData(searchSummaryProjectionQuery(target(1)).queryKey)).toBeUndefined();
  } finally { cache.clear(); }
});

it('cancelling one source does not abort another current source in the same read', async () => {
  let release: (() => void) | undefined;
  wire.mockImplementation(async (request) => {
    const body = await request.json() as SummaryBatchBody;
    await new Promise<void>((resolve) => { release = resolve; });
    expect(request.signal.aborted).toBe(false);
    return response({ items: body.targets.map(projection), unavailable: [], next_offset: null });
  });
  const cache = new QueryClient();
  try {
    const firstOptions = searchSummaryProjectionQuery(target(1));
    const first = cache.query(firstOptions).catch((error: unknown) => error);
    const second = cache.query(searchSummaryProjectionQuery(target(2)));
    await vi.waitFor(() => expect(release).toEqual(expect.any(Function)));
    await cache.cancelQueries({ queryKey: firstOptions.queryKey, exact: true });
    release?.();
    await expect(second).resolves.toEqual(projection(target(2)));
    expect(await first).toBeInstanceOf(Error);
    expect(wire).toHaveBeenCalledOnce();
  } finally { cache.clear(); }
});
