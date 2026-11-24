/** Existing opening work supplies only its own authorized Library projection, including a native-row-first race. */
import { QueryClientProvider, useQuery } from '@tanstack/react-query';
import { act, cleanup, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client, RENEW_BEFORE_END_MS } from '@/api/client';
import { feedSequences } from '@/api/cache';
import { environmentKey, environmentQuery } from '@/api/environments';
import { LiveQueryClient, queryClient } from '@/api/query';
import { libraryQuery } from '@/features/resources/Library';
import type { LibraryEntry, LibraryTarget, Me, ResourceFacts } from '@/generated/api-types';
import { applyEnvironmentFrame, environmentFeed } from '@/realtime/environmentFeed';
import type { FeedFrame } from '@/realtime/socket';
import { PageHeader } from '@/shell/Page';
import { World } from '../doubles/world';
import { deferred } from '../unit/continuation-fixtures';
import { detailEnvironment } from './resource-detail-fixtures';
import { show } from './support';

vi.mock('@/features/help/open', async (loadOriginal) => ({ ...await loadOriginal<object>(), useHelpOpener: () => vi.fn() }));
const environment = detailEnvironment;
const target: LibraryTarget = { object: { kind: 'environment', id: environment.id }, version_id: null };
const facts = (count = 3): ResourceFacts => ({ subject: { kind: 'resource', object: target.object }, as_of: new Date().toISOString(),
  dependents_complete: true, dependents: [{ relation: 'binding', count, hidden: count, visible: [], until: null }],
  uses: { kind: 'turn', coverage: { state: 'unrecorded' }, all_time: null, last_30_days: null, people: null, people_30_days: null, last_used_at: null } });
const entry = (value = facts()): LibraryEntry => ({ target, facts: value, threads: null });
const opening = (library: unknown, present = true) => ({ environment, seq: environment.revision, complete: true, since: null, jobs: [], ...(present ? { library } : {}) });
const native = (library: unknown): FeedFrame => ({ op: 'sync', id: 1, feed: environmentFeed(environment.id), prev: 0, seq: 1, data: { environment, jobs: [], library } });
const savedSession = client.session.getState();
const people = new World().people;
function person(username: string): Me { const value = people.get(username); if (value === undefined) throw new Error('The reader fixture must exist'); return value.me; }
let cache: LiveQueryClient;
let calls: string[];
function adopt(user = person('ada')) { client.adopt({ access_token: 'synthetic-library-reader', access_expires_at: new Date(Date.now() + RENEW_BEFORE_END_MS + 60_000).toISOString(), user }); }
function transport(read: () => Response | Promise<Response>, fallback = entry()) {
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init), url = new URL(request.url);
    calls.push(`${request.method} ${url.pathname}`);
    if (url.pathname === `/api/v1/environments/${environment.id}/sync`) return read();
    if (request.method === 'POST' && url.pathname === '/api/v1/resources/library') return Response.json([fallback]);
    throw new Error(`Unexpected read ${request.method} ${url.pathname}`);
  });
}
function Header() { const query = useQuery(environmentQuery(environment.id)); return query.data === undefined ? null : <PageHeader title={query.data.name} opening={target.object} />; }
function draw() { return show(<QueryClientProvider client={cache}><Header /></QueryClientProvider>); }
function libraryReads() { return calls.filter((value) => value === 'POST /api/v1/resources/library'); }
beforeEach(() => {
  calls = []; queryClient.clear(); feedSequences.clear();
  cache = new LiveQueryClient({ defaultOptions: { ...queryClient.getDefaultOptions(), queries: { ...queryClient.getDefaultOptions().queries, retry: false } } });
  adopt();
});
afterEach(() => { cleanup(); cache.clear(); queryClient.clear(); feedSequences.clear(); vi.unstubAllGlobals(); client.session.setState(savedSession); });

it('seeds the exact Library entry from the completed authorized HTTP opening', async () => {
  const value = facts(); transport(() => Response.json(opening(value)));
  await cache.query(environmentQuery(environment.id));
  expect(cache.getQueryData(libraryQuery([target]).queryKey)).toEqual([entry(value)]);
  expect(libraryReads()).toEqual([]);
});

it('publishes native Library facts before the matching Environment row can mount its header', () => {
  const value = facts(); applyEnvironmentFrame(cache, environment.id, native(value));
  expect(cache.getQueryData(environmentKey(environment.id))).toEqual(environment);
  expect(cache.getQueryData(libraryQuery([target]).queryKey)).toEqual([entry(value)]);
});

it('joins an already-running exact opening when an authorized user row mounts the actual header first', async () => {
  const began = deferred<undefined>(), released = deferred<Response>(), value = facts();
  transport(() => { began.resolve(undefined); return released.promise; });
  const pending = cache.query(environmentQuery(environment.id)); await began.promise;
  act(() => { cache.setQueryData(environmentKey(environment.id), environment); draw(); });
  expect(cache.getQueryState(environmentKey(environment.id))?.fetchStatus).toBe('fetching');
  await act(async () => { released.resolve(Response.json(opening(value))); await pending; });
  expect(await screen.findByText('Given 3 threads', { exact: true })).toBeVisible();
  expect(screen.getByText('Used – turns', { exact: true })).toBeVisible();
  expect(libraryReads()).toEqual([]);
});

for (const shape of ['legacy absent', 'explicit null', 'wrong subject'] as const) it(`keeps the existing exact standalone fallback for ${shape}`, async () => {
  const wrong = { ...facts(), subject: { kind: 'resource', object: { kind: 'environment', id: 'another-environment' } } };
  transport(() => Response.json(opening(shape === 'wrong subject' ? wrong : null, shape !== 'legacy absent')));
  draw();
  expect(await screen.findByText('Given 3 threads', { exact: true })).toBeVisible();
  expect(libraryReads()).toHaveLength(1);
});

it('retires only the exact Library entry on an explicit native denial', () => {
  const unrelatedTarget: LibraryTarget = { object: { kind: 'environment', id: 'unrelated' }, version_id: null };
  const kept: LibraryEntry[] = [{ target: unrelatedTarget, facts: { ...facts(), subject: { kind: 'resource', object: unrelatedTarget.object } }, threads: null }];
  const key = libraryQuery([target]).queryKey, unrelated = libraryQuery([unrelatedTarget]).queryKey;
  cache.setQueryData(key, [entry()]); cache.setQueryData(unrelated, kept);
  applyEnvironmentFrame(cache, environment.id, native(null));
  expect(cache.getQueryData(key)).toBeUndefined();
  expect(cache.getQueryData(unrelated)).toEqual(kept);
});

for (const shape of ['legacy environment', 'jobs only'] as const) it(`keeps current facts when a ${shape} native frame omits the Library member`, () => {
  const value = entry(), key = libraryQuery([target]).queryKey;
  cache.setQueryData(key, [value]);
  const frame: FeedFrame = { op: 'sync', id: 1, feed: environmentFeed(environment.id), prev: 0, seq: 1,
    data: { jobs: [], ...(shape === 'legacy environment' ? { environment } : {}) } };
  applyEnvironmentFrame(cache, environment.id, frame);
  expect(cache.getQueryData(key)).toEqual([value]);
});

it('cancels only the Library consumer while its shared exact opening remains in flight', async () => {
  const began = deferred<undefined>(), released = deferred<Response>(), value = facts();
  transport(() => { began.resolve(undefined); return released.promise; });
  const pending = cache.query(environmentQuery(environment.id)); await began.promise;
  const library = cache.query(libraryQuery([target])); const finished = library.catch(() => undefined);
  await cache.cancelQueries({ queryKey: libraryQuery([target]).queryKey, exact: true }); await finished;
  expect(cache.getQueryState(environmentKey(environment.id))?.fetchStatus).toBe('fetching');
  released.resolve(Response.json(opening(value))); await pending;
  expect(libraryReads()).toEqual([]);
});

it('does not publish an old reader body over current-reader metadata after headers arrived', async () => {
  const started = deferred<undefined>(), released = deferred<undefined>(), value = facts();
  transport(() => new Response(new ReadableStream<Uint8Array>({ async pull(controller) {
    started.resolve(undefined); await released.promise; controller.enqueue(new TextEncoder().encode(JSON.stringify(opening(value)))); controller.close();
  } }, { highWaterMark: 0 }), { headers: { 'Content-Type': 'application/json' } }));
  const pending = cache.query(environmentQuery(environment.id)); await started.promise;
  adopt(person('grace')); const current = entry(facts(7)); cache.setQueryData(libraryQuery([target]).queryKey, [current]);
  released.resolve(undefined); await pending.catch(() => undefined);
  expect(cache.getQueryData(libraryQuery([target]).queryKey)).toEqual([current]);
});

it('retains the canonical fallback after the existing opening rejects', async () => {
  const began = deferred<undefined>(), released = deferred<Response>();
  transport(() => { began.resolve(undefined); return released.promise; });
  const pending = cache.query(environmentQuery(environment.id)); const finished = pending.catch(() => undefined); await began.promise;
  act(() => { cache.setQueryData(environmentKey(environment.id), environment); draw(); });
  await act(async () => { released.resolve(Response.json({ type: 'orch:error:source.offline', code: 'source.offline', title: 'Read failed', status: 503, detail: 'Unavailable', request_id: 'synthetic-opening' }, { status: 503 })); await finished; });
  expect(await screen.findByText('Given 3 threads', { exact: true })).toBeVisible();
  expect(libraryReads()).toHaveLength(1);
});
