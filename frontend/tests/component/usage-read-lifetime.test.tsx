/** A bounded analytics read survives first-render churn while native suppliers and reader changes still win. */
import { CancelledError, QueryClient, QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, screen, waitFor } from '@testing-library/react';
import { StrictMode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Client from '@/api/client';
import { client } from '@/api/client';
import { supplyOverviewPart } from '@/api/overviewParts';
import { scheduledReads } from '@/api/readScheduler';
import { usageQuery, type UsageParams } from '@/api/usage';
import type { UsageAnalytics } from '@/generated/api-types';
import { setLocale } from '@/i18n';
import { ACME } from '../doubles/fixtures';
import { nodeOverview } from '../doubles/screen-openings';
import { World } from '../doubles/world';
import { deferred } from './deferred';
import { show } from './support';

interface Request { params: { query: Record<string, unknown> }; signal?: AbortSignal }
const wire = vi.hoisted(() => ({ get: vi.fn<(_path: string, _request: Request) => Promise<{ data: UsageAnalytics; response: Response }>>() }));
vi.mock('@/api/client', async (load) => ({ ...await load<typeof Client>(), api: { GET: wire.get } }));
const params: UsageParams = { scope: `node:${ACME.web}`, from: '2026-10-01', to: '2026-10-07', breakdowns: [], filters: {} };
const reply = (data: UsageAnalytics) => ({ data, response: new Response(null, { status: 200 }) });
async function nativeUsage(): Promise<UsageAnalytics> {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The native usage needs its reader');
  const opening = await nodeOverview(world, person, ACME.web, new URLSearchParams({ from: params.from, to: params.to, breakdowns: '' }));
  if (opening.usage.status !== 'ready') throw new Error('The native overview needs its usage projection');
  return opening.usage.data;
}
function Figures({ request = params }: { request?: UsageParams }) {
  const result = useQuery(usageQuery(request));
  return <output aria-label="Usage">{result.data?.totals.turns ?? 'pending'}</output>;
}
const before = client.session.getState();
const caches = new Set<QueryClient>();
function cache() {
  const value = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  caches.add(value); return value;
}
beforeEach(() => { wire.get.mockReset(); scheduledReads.advance(); client.session.setState({ ...before, status: 'signed_in', userId: 'usage-reader', readerVersion: 1 }); });
afterEach(async () => { cleanup(); for (const value of caches) value.clear(); caches.clear(); scheduledReads.advance(); client.session.setState(before); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) {
  it(`keeps one pending analytics request through Strict Mode and a real remount in ${locale}`, async () => {
    await setLocale(locale);
    const data = await nativeUsage(), pending = deferred<UsageAnalytics>(), held = cache();
    wire.get.mockImplementation(() => pending.promise.then(reply));
    const draw = () => <QueryClientProvider client={held}><StrictMode><Figures /></StrictMode></QueryClientProvider>;
    const first = show(draw());
    await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(1));
    expect(wire.get.mock.calls[0]?.[1]).not.toHaveProperty('signal');
    first.unmount();
    show(draw());
    expect(wire.get).toHaveBeenCalledTimes(1);
    await act(async () => { pending.resolve(data); await Promise.resolve(); });
    await waitFor(() => expect(screen.getByLabelText('Usage')).toHaveTextContent(String(data.totals.turns)));
    expect(held.getQueryData(usageQuery(params).queryKey)).toEqual(data);
    expect(wire.get).toHaveBeenCalledTimes(1);
  });

  it(`reads once more for a real filter change and keeps late old-filter data in its own cache in ${locale}`, async () => {
    await setLocale(locale);
    const data = await nativeUsage(), initial = deferred<UsageAnalytics>(), next = deferred<UsageAnalytics>(), held = cache();
    const changed: UsageAnalytics = { ...data, totals: { ...data.totals, turns: data.totals.turns + 10 } };
    wire.get.mockImplementation((_path, request) => (request.params.query.model === 'another' ? next : initial).promise.then(reply));
    const draw = (request: UsageParams) => <QueryClientProvider client={held}><Figures request={request} /></QueryClientProvider>;
    const view = show(draw(params));
    await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(1));
    const filtered = { ...params, filters: { model: 'another' } };
    view.rerender(draw(filtered));
    await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(2));
    await act(async () => { next.resolve(changed); await Promise.resolve(); });
    await waitFor(() => expect(screen.getByLabelText('Usage')).toHaveTextContent(String(changed.totals.turns)));
    await act(async () => { initial.resolve(data); await Promise.resolve(); });
    await waitFor(() => expect(held.getQueryData(usageQuery(params).queryKey)).toEqual(data));
    expect(screen.getByLabelText('Usage')).toHaveTextContent(String(changed.totals.turns));
  });
}

it.each(['reader', 'source'] as const)('does not publish a late analytics answer after the %s changes', async (change) => {
  const data = await nativeUsage(), pending = deferred<UsageAnalytics>(), held = cache();
  wire.get.mockImplementation(() => pending.promise.then(reply));
  const request = held.query(usageQuery(params));
  const rejected = expect(request).rejects.toBeInstanceOf(CancelledError);
  await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(1));
  if (change === 'reader') client.session.setState({ readerVersion: 2 });
  else scheduledReads.advance();
  pending.resolve(data); await rejected;
  expect(held.getQueryData(usageQuery(params).queryKey)).toBeUndefined();
});

for (const older of ['ready', 'failed'] as const) it.each(['ready', 'failed'] as const)(`keeps a newer native %s part when an older ${older} standalone read finishes`, async (status) => {
  const data = await nativeUsage(), pending = deferred<UsageAnalytics>(), held = cache(), key = usageQuery(params).queryKey;
  wire.get.mockImplementation(() => pending.promise.then(reply));
  const request = held.query(usageQuery(params));
  const outcome = request.then((value) => ({ value, error: null }), (error: unknown) => ({ value: undefined, error }));
  await waitFor(() => expect(wire.get).toHaveBeenCalledTimes(1));
  const newer: UsageAnalytics = { ...data, totals: { ...data.totals, turns: data.totals.turns + 20 } };
  supplyOverviewPart(held, key, status === 'ready' ? { status, data: newer } : { status, problem: {
    type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'No access', detail: 'Usage is unavailable.', request_id: 'native-usage-refusal',
  } });
  scheduledReads.supply(['native-overview'], [key]);
  const suppliedError = held.getQueryState(key)?.error;
  if (older === 'ready') pending.resolve(data);
  else pending.reject(new Error('The older transport failed'));
  const result = await outcome;
  if (status === 'ready') {
    expect(result.value).toEqual(newer); expect(result.error).toBeNull();
    expect(held.getQueryData(key)).toEqual(newer);
  } else {
    expect(result.error).toBe(suppliedError);
    expect(held.getQueryState(key)).toMatchObject({ status: 'error', data: undefined });
  }
  expect(held.getQueryState(key)?.fetchStatus).toBe('idle');
});
