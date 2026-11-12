/** The actual native-read query follows server timing, never a paid refresh timer. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import type { PropsWithChildren } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { resourceSummaryQuery, resourceSummaryKey } from '@/api/summaries';
import type { SummarySnapshot } from '@/generated/api-types';
import { seedSummaryOpening } from '@/api/summarySnapshots';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => { cleanup(); vi.useRealTimers(); vi.unstubAllGlobals(); wire.mockReset(); });

it('waits for a future native due time and uses the returned pace only while pending', async () => {
  // This case controls the transport clock; paint coalescing is covered by the staged-opening case.
  vi.stubGlobal('requestAnimationFrame', undefined);
  vi.useFakeTimers(); vi.setSystemTime(new Date('2026-11-08T22:00:00Z'));
  const target = { kind: 'document' as const, id: '00000000-0000-4000-8202-000000000001' };
  const base: SummarySnapshot = { target, root_org_id: 'root', inspecting: false, revision: 1, state: 'scheduled', text: 'Previous paragraph.',
    producer: null, input: null, manual_problem: null, updated_at: null, route: null, problem: null,
    wait: { reason: 'interval', until: '2026-11-08T22:15:00Z' }, poll_after_ms: 900_000 };
  const requests: Request[] = [];
  wire.mockImplementation((request) => {
    requests.push(request);
    const body = requests.length === 1 ? base : requests.length === 2 ? { ...base, state: 'updating', wait: null, poll_after_ms: 7000 }
      : { ...base, state: 'fresh', wait: null, poll_after_ms: null };
    return Promise.resolve(new Response(JSON.stringify({ items: [{ ...body, full: body }], unavailable: [], next_offset: null }), { status: 200, headers: { 'Content-Type': 'application/json' } }));
  });
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: PropsWithChildren) => <QueryClientProvider client={cache}>{children}</QueryClientProvider>;
  const hook = renderHook(() => useQuery(resourceSummaryQuery(target)), { wrapper });
  try {
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(wire).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(120_000); });
    expect(wire).toHaveBeenCalledTimes(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(780_000); });
    expect(wire).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(6999); });
    expect(wire).toHaveBeenCalledTimes(2);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(wire).toHaveBeenCalledTimes(3);
    await act(async () => { await vi.advanceTimersByTimeAsync(60_000); });
    expect(wire).toHaveBeenCalledTimes(3);
    expect(requests.every((request) => request.method === 'POST' && new URL(request.url).pathname === '/api/v1/summaries/batch')).toBe(true);
  } finally { hook.unmount(); cache.clear(); }
});

it.each([false, true])('adopts the chat opening once, honoring invalidation and later sole mounts (%s)', async (invalidated) => {
  const target = { kind: 'chat_title', id: '00000000-0000-4000-8202-000000000001' } as const;
  const snapshot: SummarySnapshot = { target, root_org_id: 'root', inspecting: false, revision: 1, state: 'fresh', text: 'The admitted title.',
    producer: null, input: null, manual_problem: null, updated_at: null, route: null, problem: null, wait: null, poll_after_ms: null };
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  seedSummaryOpening(cache, snapshot);
  wire.mockImplementation(() => Promise.resolve(new Response(JSON.stringify({ items: [{ ...snapshot, full: snapshot }], unavailable: [], next_offset: null }), { status: 200, headers: { 'Content-Type': 'application/json' } })));
  if (invalidated) cache.getQueryCache().find({ queryKey: resourceSummaryKey(target) })?.invalidate();
  const wrapper = ({ children }: PropsWithChildren) => <QueryClientProvider client={cache}>{children}</QueryClientProvider>;
  const first = renderHook(() => useQuery(resourceSummaryQuery(target)), { wrapper });
  await act(async () => { await Promise.resolve(); });
  expect(first.result.current.data?.text).toBe('The admitted title.');
  if (invalidated) await vi.waitFor(() => { expect(wire).toHaveBeenCalledOnce(); });
  else expect(wire).not.toHaveBeenCalled();
  first.unmount();
  const next = renderHook(() => useQuery(resourceSummaryQuery(target)), { wrapper });
  try { await vi.waitFor(() => { expect(wire).toHaveBeenCalledTimes(invalidated ? 2 : 1); }); }
  finally { next.unmount(); cache.clear(); }
});
