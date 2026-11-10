/** The actual native-read query follows server timing, never a paid refresh timer. */
import { act, cleanup, renderHook } from '@testing-library/react';
import { QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import type { PropsWithChildren } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { resourceSummaryQuery } from '@/api/summaries';
import type { SummarySnapshot } from '@/generated/api-types';

const wire = vi.hoisted(() => vi.fn<(request: Request) => Promise<Response>>());
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof Client>();
  const client = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: wire });
  return { ...actual, client, api: client.api };
});
afterEach(() => { cleanup(); vi.useRealTimers(); wire.mockReset(); });

it('waits for a future native due time and uses the returned pace only while pending', async () => {
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
    return Promise.resolve(new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } }));
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
    expect(requests.every((request) => request.method === 'GET')).toBe(true);
  } finally { hook.unmount(); cache.clear(); }
});
