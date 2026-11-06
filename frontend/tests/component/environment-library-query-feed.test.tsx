/** The resource screen's primary feed carries Library facts before it retires the HTTP opening. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { feedSequences } from '@/api/cache';
import { client, RENEW_BEFORE_END_MS } from '@/api/client';
import { environmentKey, environmentQuery } from '@/api/environments';
import { LiveQueryClient, queryClient } from '@/api/query';
import { libraryQuery } from '@/features/resources/Library';
import type { LibraryEntry, LibraryTarget, ResourceFacts } from '@/generated/api-types';
import { applyQueryFrame } from '@/realtime/queryFeeds';
import type { FeedFrame } from '@/realtime/socket';
import { World } from '../doubles/world';
import { deferred } from '../unit/continuation-fixtures';
import { detailEnvironment } from './resource-detail-fixtures';

const environment = detailEnvironment;
const target: LibraryTarget = { object: { kind: 'environment', id: environment.id }, version_id: null };
const facts = (count = 3): ResourceFacts => ({ subject: { kind: 'resource', object: target.object }, as_of: new Date().toISOString(),
  dependents_complete: true, dependents: [{ relation: 'binding', count, hidden: count, visible: [], until: null }], uses: null });
const entry = (value = facts()): LibraryEntry => ({ target, facts: value, threads: null });
const native = (data: Record<string, unknown>): FeedFrame => ({ op: 'sync', id: 1, feed: `env:${environment.id}`, prev: 0, seq: 1, data });
const savedSession = client.session.getState();
let cache: LiveQueryClient;
beforeEach(() => {
  queryClient.clear(); feedSequences.clear();
  cache = new LiveQueryClient({ defaultOptions: { ...queryClient.getDefaultOptions(), queries: { ...queryClient.getDefaultOptions().queries, retry: false } } });
  const person = new World().people.get('ada'); if (person === undefined) throw new Error('The current reader fixture must exist');
  client.adopt({ access_token: 'synthetic-library-reader', access_expires_at: new Date(Date.now() + RENEW_BEFORE_END_MS + 60_000).toISOString(), user: person.me });
});
afterEach(() => { cache.clear(); queryClient.clear(); feedSequences.clear(); vi.unstubAllGlobals(); client.session.setState(savedSession); });

it('seeds the primary envelope before publishing its current Environment', () => {
  const value = facts(); applyQueryFrame(cache, environmentKey(environment.id), native({ environment, jobs: [], library: value }));
  expect(cache.getQueryData(environmentKey(environment.id))).toEqual(environment);
  expect(cache.getQueryData(libraryQuery([target]).queryKey)).toEqual([entry(value)]);
});

it('lets the existing Library waiter use the native seed before its opening is cancelled', async () => {
  const began = deferred<undefined>(), released = deferred<Response>(), value = facts();
  let posts = 0;
  vi.stubGlobal('fetch', async (input: RequestInfo | URL, init?: RequestInit) => {
    const request = input instanceof Request ? input : new Request(input, init), path = new URL(request.url).pathname;
    if (path === `/api/v1/environments/${environment.id}/sync`) { began.resolve(undefined); return released.promise; }
    if (request.method === 'POST' && path === '/api/v1/resources/library') { posts += 1; return Response.json([entry(facts(99))]); }
    throw new Error(`Unexpected request ${request.method} ${path}`);
  });
  const opening = cache.query(environmentQuery(environment.id)); const ended = opening.catch(() => undefined); await began.promise;
  const waiting = cache.query(libraryQuery([target]));
  applyQueryFrame(cache, environmentKey(environment.id), native({ environment, jobs: [], library: value }));
  released.resolve(Response.json({ environment, jobs: [], library: facts(99), seq: environment.revision, complete: true, since: null }));
  await ended;
  expect(await waiting).toEqual([entry(value)]);
  expect(posts).toBe(0);
  expect(cache.getQueryData(environmentKey(environment.id))).toEqual(environment);
});

for (const shape of ['current environment', 'Library only'] as const) it(`retires an explicitly denied primary member in a ${shape} envelope`, () => {
  const key = libraryQuery([target]).queryKey;
  cache.setQueryData(key, [entry()]);
  applyQueryFrame(cache, environmentKey(environment.id), native({ ...(shape === 'current environment' ? { environment } : {}), jobs: [], library: null }));
  expect(cache.getQueryData(key)).toBeUndefined();
});

for (const shape of ['legacy environment', 'jobs only'] as const) it(`keeps current facts when the primary ${shape} envelope omits the member`, () => {
  const value = entry(), key = libraryQuery([target]).queryKey;
  cache.setQueryData(key, [value]);
  applyQueryFrame(cache, environmentKey(environment.id), native({ ...(shape === 'legacy environment' ? { environment } : {}), jobs: [] }));
  expect(cache.getQueryData(key)).toEqual([value]);
});

it('leaves metadata untouched when the existing primary Environment identity guard rejects its envelope', () => {
  const value = entry(), key = libraryQuery([target]).queryKey;
  cache.setQueryData(key, [value]);
  applyQueryFrame(cache, environmentKey(environment.id), native({ environment: { ...environment, id: 'another-environment' }, library: null }));
  expect(cache.getQueryData(key)).toEqual([value]);
});
