/** Exact native causation joins echoes without discarding a different post-start change. No timing sleeps. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { applyChanges, findAct, userEventChanges } from '@/api/acts';
import { client, createApiClient, ok, type Act } from '@/api/client';
import { meta } from '../doubles/fixtures';

const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
const reader = client.session.getState(), releases: (() => void)[] = [];
afterEach(() => { for (const release of releases.splice(0)) release(); cache.clear(); client.session.setState(reader); });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve };
}
function observe(read: () => Promise<ReturnType<typeof meta>>) {
  cache.setQueryData(['meta'], meta());
  const observer = new QueryObserver(cache, { queryKey: ['meta'], queryFn: read, staleTime: Infinity });
  releases.push(observer.subscribe(() => undefined)); return observer;
}
async function actor(requestId: string) {
  const act: Act = { method: 'PUT', path: '/admin/settings/system.banner', requestId, answer: { key: 'system.banner', value: null } };
  const found = findAct(act.method, act.path, act);
  if (found === null) throw new Error('The native setting act is unregistered');
  await applyChanges(found.changes, cache);
}
const hint = (requestId?: string) => applyChanges(userEventChanges('meta_changed', { what: 'meta_changed', ...(requestId === undefined ? {} : { request_id: requestId }) }), cache);

it('takes the cause from the native response header and preserves it on the real successful-act callback', async () => {
  const seen: Act[] = [], requests: Request[] = [];
  const owned = createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: (request) => {
    requests.push(request); return Promise.resolve(Response.json({ key: 'system.banner', value: null }, { headers: { 'X-Request-Id': 'native-cause' } }));
  } });
  owned.onAct((act) => { seen.push(act); });
  await ok(owned.api.PUT('/admin/settings/{key}', { params: { path: { key: 'system.banner' } }, body: { value: null } }));
  expect(seen).toHaveLength(1); expect(seen[0]).toMatchObject({ requestId: 'native-cause', method: 'PUT', path: '/admin/settings/system.banner' });
  expect(requests[0]?.headers.get('X-Request-Id')).toBeNull();
});

for (const first of ['actor', 'hint'] as const) it(`joins a concurrent echo and a delayed same-ID echo when ${first} arrives first`, async () => {
  const gate = deferred<ReturnType<typeof meta>>(), started = deferred<undefined>();
  const after = meta({ may_create_org: true });
  let count = 0;
  const read = vi.fn(() => { count += 1; if (count === 1) { started.resolve(undefined); return gate.promise; } return Promise.resolve(after); });
  observe(read);
  const initial = first === 'actor' ? actor('cause-A') : hint('cause-A'); await started.promise;
  const echo = first === 'actor' ? hint('cause-A') : actor('cause-A');
  gate.resolve(after); await Promise.all([initial, echo]);
  expect(read).toHaveBeenCalledOnce(); expect(cache.getQueryData(['meta'])).toEqual(after);
  await hint('cause-A'); await actor('cause-A'); expect(read).toHaveBeenCalledOnce();
});

it('queues a genuinely newer cause during a repair, while both causes own only one read despite their echoes', async () => {
  const first = deferred<ReturnType<typeof meta>>(), second = deferred<ReturnType<typeof meta>>();
  const started = deferred<undefined>(), newer = deferred<undefined>();
  let count = 0;
  const read = vi.fn(() => {
    count += 1;
    if (count === 1) { started.resolve(undefined); return first.promise; }
    if (count === 2) { newer.resolve(undefined); return second.promise; }
    return Promise.resolve(meta({ may_create_org: false }));
  });
  observe(read);
  const a = actor('cause-A'); await started.promise;
  const echoA = hint('cause-A'), b = hint('cause-B'), echoB = actor('cause-B');
  first.resolve(meta({ may_create_org: true })); await newer.promise;
  await Promise.all([a, echoA]); expect(read).toHaveBeenCalledTimes(2);
  second.resolve(meta({ may_create_org: false })); await Promise.all([b, echoB]);
  await hint('cause-A'); await hint('cause-B');
  expect(read).toHaveBeenCalledTimes(2); expect(cache.getQueryData(['meta'])).toEqual(meta({ may_create_org: false }));
});

it('reads once for distinct committed causes already present in the same received batch', async () => {
  const read = vi.fn(() => Promise.resolve(meta())); observe(read);
  await applyChanges([...userEventChanges('meta_changed', { request_id: 'A' }), ...userEventChanges('meta_changed', { request_id: 'B' })], cache);
  await actor('A'); await actor('B'); expect(read).toHaveBeenCalledOnce();
});

it('bounds completed retention at 256 IDs and conservatively refreshes an evicted, unknown or legacy cause', async () => {
  const read = vi.fn(() => Promise.resolve(meta())); observe(read);
  for (let at = 0; at < 257; at += 1) await hint(`cause-${at}`);
  expect(read).toHaveBeenCalledTimes(257);
  await actor('cause-256'); expect(read).toHaveBeenCalledTimes(257);
  await actor('cause-0'); expect(read).toHaveBeenCalledTimes(258);
  await hint('unknown'); await hint(); await hint(); expect(read).toHaveBeenCalledTimes(261);
});

it('never evicts an active cause while more than 256 different causes wait behind it', async () => {
  const first = deferred<ReturnType<typeof meta>>(), second = deferred<ReturnType<typeof meta>>();
  const started = deferred<undefined>(), newer = deferred<undefined>();
  let count = 0;
  const read = vi.fn(() => {
    count += 1;
    if (count === 1) { started.resolve(undefined); return first.promise; }
    newer.resolve(undefined); return second.promise;
  });
  observe(read); const a = actor('active-A'); await started.promise;
  const pending = Array.from({ length: 257 }, (_, index) => hint(`new-${index}`)), echo = hint('active-A');
  first.resolve(meta()); await newer.promise; await Promise.all([a, echo]);
  // The echo finished with A while the genuinely newer batch remains explicitly held.
  expect(read).toHaveBeenCalledTimes(2); second.resolve(meta({ may_create_org: true })); await Promise.all(pending);
  expect(read).toHaveBeenCalledTimes(2);
});

for (const boundary of ['reader', 'query'] as const) it(`forgets completed cause ownership across the ${boundary} lifetime`, async () => {
  const read = vi.fn(() => Promise.resolve(meta())); observe(read); await actor('same-id'); expect(read).toHaveBeenCalledOnce();
  if (boundary === 'reader') client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 });
  else { for (const release of releases.splice(0)) release(); cache.clear(); observe(read); }
  await hint('same-id'); expect(read).toHaveBeenCalledTimes(2);
});
