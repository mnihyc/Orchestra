/** The public scope string reaches the native report, ledger and CSV readers unchanged. */
import { QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { fetchEventsPage, fetchSeriesCsv, usageQuery } from '@/api/usage';
interface Request { params: { query: Record<string, unknown> }; signal?: AbortSignal; headers?: HeadersInit; parseAs?: string }
const wire = vi.hoisted(() => ({ get: vi.fn<(path: string, request: Request) => Promise<{ data: unknown; response: Response }>>() }));
vi.mock('@/api/client', async (original) => { const actual = await original<typeof Client>(); return { ...actual, api: { ...actual.api, GET: wire.get } }; });
const caches: QueryClient[] = [];
beforeEach(() => { wire.get.mockReset(); wire.get.mockResolvedValue({ data: { scope: 'system' }, response: new Response(null, { status: 200 }) }); });
afterEach(() => { caches.forEach((cache) => cache.clear()); caches.length = 0; });
const params = { scope: 'system', from: '2026-10-01', to: '2026-10-07', breakdowns: [], filters: {} };
it('uses the existing figures route with System and an abortable privileged flight', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache); await cache.query(usageQuery(params));
  expect(wire.get.mock.calls[0]?.[0]).toBe('/usage'); expect(wire.get.mock.calls[0]?.[1].params.query.scope).toBe('system'); expect(wire.get.mock.calls[0]?.[1].signal).toBeInstanceOf(AbortSignal);
});
it('keeps an ordinary bounded usage flight independent of a view remount', async () => {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache); await cache.query(usageQuery({ ...params, scope: 'me' }));
  expect(wire.get.mock.calls[0]?.[1]).not.toHaveProperty('signal');
});
it('retains System and the org filter in ledger reads without a personal fallback', async () => {
  await fetchEventsPage({ ...params, filters: { org_id: 'chosen-org' } }, null);
  expect(wire.get.mock.calls[0]?.[0]).toBe('/usage/events'); expect(wire.get.mock.calls[0]?.[1].params.query).toMatchObject({ scope: 'system', org_id: 'chosen-org' });
});
it('exports the current System series from the same native scope', async () => {
  wire.get.mockResolvedValue({ data: 'day,credits\n2026-10-01,1', response: new Response(null, { status: 200 }) }); expect(await fetchSeriesCsv(params)).toContain('day,credits');
  expect(wire.get.mock.calls[0]?.[1].params.query.scope).toBe('system'); expect(wire.get.mock.calls[0]?.[1].parseAs).toBe('text');
});
