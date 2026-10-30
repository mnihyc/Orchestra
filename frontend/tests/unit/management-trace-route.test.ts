/** The actual Management route keeps Audit URL filters until each section applies its own schema. */
import { InfiniteQueryObserver, QueryClient } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientApi from '@/api/client';
import { trailQuery, wireFilters } from '@/api/trace';
import { readSearch, withSearch } from '@/features/admin/search';
import { AUDIT_KINDS, traceHref, traceSearch } from '@/features/trace/params';
import type { AuditEntryPage } from '@/generated/api-types';
import { Route } from '@/routes/admin/$section/index';

interface ReadOptions { params: { query: Record<string, unknown>; path?: Record<string, string> }; signal: AbortSignal }
const calls = vi.hoisted(() => ({ read: vi.fn<(path: string, options: ReadOptions) => Promise<{ data: AuditEntryPage; response: Response }>>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof ClientApi>();
  return { ...actual, api: { ...actual.api, GET: calls.read } };
});
// Keep the real file route and its validator; rendering unrelated Management sections is unnecessary here.
vi.mock('@/features/admin/Management', () => ({ Management: () => null }));

const actor = '01950000-0000-7000-8000-000000000001';
const system = { type: 'system' as const, name: 'System' };
const validate = (raw: Record<string, unknown>) => {
  const read = Route.options.validateSearch;
  if (typeof read !== 'function') throw new Error('The actual Management route must validate its search.');
  return read(raw);
};
const fromUrl = (href: string) => validate(Object.fromEntries(new URL(href, 'https://orch.test').searchParams));
const reload = (search: Record<string, unknown>) => fromUrl('/admin/audit?' + new URLSearchParams(
  Object.entries(search).map(([key, value]) => [key, String(value)]),
).toString());
let cache: QueryClient;
beforeEach(() => {
  calls.read.mockReset();
  cache = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  calls.read.mockResolvedValue({ data: { items: [], next_cursor: null }, response: new Response(null, { status: 200 }) });
});
afterEach(() => { cache.clear(); });

it('retains the system person entry and explicit included-agent choice through the actual route and reload', () => {
  for (const include_agents of [true, false]) {
    const search = fromUrl(traceHref(system, actor, { include_agents }));
    expect(traceSearch(search)).toEqual({ actor, include_agents });
    expect(traceSearch(reload(search))).toEqual({ actor, include_agents });
    expect(wireFilters(traceSearch(search))).toEqual({ actor, include_agents: String(include_agents) });
  }
  expect(traceSearch(fromUrl(traceHref(system, actor)))).toEqual({ actor, include_agents: true });
});

it('keeps Audit kinds, exact actions, date bounds and grouping without turning unknown kinds into a broad query', () => {
  for (const kind of [...AUDIT_KINDS, 'future_kind']) {
    const search = fromUrl(traceHref(system, actor, {
      kind, action: 'permission.removed', include_agents: false, group: 'kind',
      from: '2026-10-01T01:02:03.000Z', to: '2026-10-13',
    }));
    expect(traceSearch(reload(search))).toEqual({ actor, kind, action: 'permission.removed',
      include_agents: false, group: 'kind', from: '2026-10-01T01:02:03.000Z', to: '2026-10-13' });
    expect(wireFilters(traceSearch(search))).toEqual({ actor, kind, action: 'permission.removed',
      include_agents: 'false', from: '2026-10-01T01:02:03.000Z', to: '2026-10-13T23:59:59.999Z' });
  }
});

it('sends preserved filters on every system page and keeps direct-only, other actors and node trails in separate caches', async () => {
  const filters = traceSearch(fromUrl(traceHref(system, actor, { kind: 'access', action: 'permission.removed', group: 'kind' })));
  calls.read.mockResolvedValueOnce({ data: { items: [], next_cursor: 'native-next' }, response: new Response(null, { status: 200 }) });
  const options = trailQuery(system, filters), observer = new InfiniteQueryObserver(cache, { ...options, enabled: false });
  await observer.refetch(); await observer.fetchNextPage();
  expect(calls.read.mock.calls.map(([path, value]) => [path, value.params])).toEqual([
    ['/admin/audit', { query: { actor, include_agents: true, kind: 'access', action: 'permission.removed', limit: 40 } }],
    ['/admin/audit', { query: { actor, include_agents: true, kind: 'access', action: 'permission.removed', limit: 40, cursor: 'native-next' } }],
  ]);
  expect(calls.read.mock.calls.every(([, value]) => value.signal instanceof AbortSignal)).toBe(true);
  observer.destroy();
  const keys = [options.queryKey, trailQuery(system, { ...filters, include_agents: false }).queryKey,
    trailQuery(system, { ...filters, actor: '01950000-0000-7000-8000-000000000002' }).queryKey,
    trailQuery({ type: 'node', id: 'place', name: 'Place', kind: 'org', orgId: 'place', access: undefined }, filters).queryKey];
  expect(new Set(keys.map((key) => JSON.stringify(key))).size).toBe(4);
  expect(options.staleTime).toBe(0); expect(options.gcTime).toBe(0);
});

it('keeps every other Management page on its existing narrow runtime and form rules', () => {
  for (const kind of ['environment', 'provider', 'access', 'future_kind', 'both']) {
    const raw = { kind, host: 'host-id', q: ' ada ', status: 'active', from: 'tomorrow',
      include_agents: 'true', group: 'kind', bogus: 'ignored' };
    // This is the exact per-page narrowing AdminRoute passes to Management/SectionProps.
    expect(readSearch({ ...validate(raw) })).toEqual(readSearch(raw));
    expect(readSearch({ ...validate(raw) }).kind).toBe(kind === 'environment' || kind === 'provider' ? kind : undefined);
  }
  expect(withSearch({ kind: 'provider', host: 'host-id' }, { kind: undefined })).toEqual({ host: 'host-id' });
  expect(validate({ include_agents: 'sometimes', group: 'actor', actor: {}, kind: ['access'], bogus: 'ignored' })).toEqual({});
});
