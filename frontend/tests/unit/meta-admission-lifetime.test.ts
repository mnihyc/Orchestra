/** Real Query observers/client restoration, with explicitly released transport replies and no timing sleeps. */
import { CancelledError, QueryObserver } from '@tanstack/react-query';
import { isRedirect } from '@tanstack/react-router';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import { client, ApiError } from '@/api/client';
import { metaKey, metaQuery, refreshMetaForSignIn } from '@/api/auth';
import { queryClient } from '@/api/query';
import { admit } from '@/routes/-access';
import type { AuthResult, Meta } from '@/generated/api-types';
import { me, meta } from '../doubles/fixtures';

const transport = vi.hoisted(() => ({ send: vi.fn<(request: Request) => Promise<Response>>() }));
vi.mock('@/api/client', async (load) => {
  const actual = await load<typeof ClientModule>();
  const owned = actual.createApiClient({ baseUrl: 'http://orch.test/api/v1', fetch: (request) => transport.send(request) });
  return { ...actual, client: owned, api: owned.api };
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}
interface Read { request: Request; reply: ReturnType<typeof deferred<Response>> }
type Outcome = { ok: true } | { ok: false; error: unknown };
const first = me(1, 'first', 'First reader'), second = me(2, 'second', 'Second reader');
const visit = { needs: 'signed_in' as const, href: '/thread/owned', pathname: '/thread/owned', onward: undefined };
const defaults = queryClient.getDefaultOptions();
let reads: Read[], waiting: Map<number, ReturnType<typeof deferred<Read>>>;
let restoreReply: ReturnType<typeof deferred<Response>>, restoreStarted: ReturnType<typeof deferred<undefined>>;
let release: (() => void)[], pending: Promise<unknown>[], freshness: Promise<void>[];

function grant(person = first): AuthResult {
  return { user: person, access_token: `synthetic-${person.username}`, access_expires_at: new Date(Date.now() + 900_000).toISOString() };
}
function observed(work: Promise<unknown>): Promise<Outcome> {
  const result: Promise<Outcome> = work.then(() => ({ ok: true }), (error: unknown) => ({ ok: false, error }));
  pending.push(result);
  return result;
}
function read(number: number): Promise<Read> {
  const held = reads[number - 1];
  if (held !== undefined) return Promise.resolve(held);
  const gate = waiting.get(number) ?? deferred<Read>(); waiting.set(number, gate); return gate.promise;
}
function observeFrame() {
  const observer = new QueryObserver(queryClient, metaQuery);
  const off = observer.subscribe(() => undefined); release.push(off); return off;
}
function refreshOnSignIn(detach?: () => void) {
  release.push(client.onSession((event) => {
    if (event.type === 'signed_in') {
      // Restoration can unmount AuthFrame before the new shell's Meta observer subscribes.
      detach?.(); freshness.push(refreshMetaForSignIn());
    }
  }));
}
async function restoreAs(person = first) {
  const restored = client.restore(); pending.push(restored);
  await restoreStarted.promise;
  restoreReply.resolve(Response.json(grant(person)));
  await expect(restored).resolves.toBe(true);
}
async function answer(number: number, data: Meta) {
  const fetch = queryClient.getQueryCache().find({ queryKey: metaKey, exact: true })?.promise;
  (await read(number)).reply.resolve(Response.json(data));
  if (fetch !== undefined) await fetch;
}
function observers() { return queryClient.getQueryCache().find({ queryKey: metaKey, exact: true })?.getObserversCount() ?? 0; }

beforeEach(() => {
  vi.useFakeTimers();
  queryClient.clear(); queryClient.setDefaultOptions({ ...defaults, queries: { ...defaults.queries, retry: false } });
  client.session.setState({ status: 'unknown', userId: null, accessEndsAt: null, readerVersion: 0 });
  reads = []; waiting = new Map(); release = []; pending = []; freshness = [];
  restoreReply = deferred<Response>(); restoreStarted = deferred<undefined>();
  transport.send.mockReset();
  transport.send.mockImplementation((request) => {
    const path = new URL(request.url).pathname;
    if (path === '/api/v1/meta') {
      const next = { request, reply: deferred<Response>() }; reads.push(next); waiting.get(reads.length)?.resolve(next); return next.reply.promise;
    }
    if (path === '/api/v1/auth/refresh') { restoreStarted.resolve(undefined); return restoreReply.promise; }
    if (path === '/api/v1/auth/logout') return Promise.resolve(new Response(null, { status: 204 }));
    throw new Error(`Unexpected native request ${path}`);
  });
});
afterEach(async () => {
  for (const stop of release.splice(0)) stop();
  queryClient.clear();
  // Settle even the deliberately held/aborted replies without creating a real request or timer wait.
  transport.send.mockImplementation((request) => Promise.resolve(new URL(request.url).pathname.endsWith('/logout')
    ? new Response(null, { status: 204 }) : Response.json(meta({ may_create_org: false }))));
  for (const entry of reads) entry.reply.resolve(Response.json(meta({ may_create_org: false })));
  restoreReply.resolve(Response.json(grant()));
  await Promise.allSettled([...pending, ...freshness]);
  await client.signOut();
  queryClient.clear(); queryClient.setDefaultOptions(defaults); vi.clearAllTimers(); vi.useRealTimers();
});

it.each(['restore first', 'Meta first'] as const)('retains actual admission across the frame handoff and refreshes caller-specific Meta once (%s)', async (order) => {
  const admission = observed(admit(visit));
  const initial = await read(1), detach = observeFrame(); refreshOnSignIn(detach);
  expect(initial.request.headers.has('authorization')).toBe(false);
  expect(observers()).toBe(2);
  if (order === 'Meta first') await answer(1, meta({ name: 'Admission system', may_create_org: false }));
  await restoreAs();
  const shell = observeFrame();
  if (order === 'restore first') {
    expect(initial.request.signal.aborted).toBe(false); expect(reads).toHaveLength(1);
    await answer(1, meta({ name: 'Admission system', may_create_org: false }));
  }
  // This unchanged assertion fails on the predecessor control sequence; no error is swallowed.
  expect(await admission).toEqual({ ok: true });
  const current = await read(2);
  expect(current.request.headers.get('authorization')).toBe('Bearer synthetic-first');
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false);
  await answer(2, meta({ name: 'Admission system', may_create_org: true }));
  await Promise.all(freshness);
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(true);
  expect(observers()).toBe(1); shell(); expect(observers()).toBe(0);
  await admit(visit); client.adopt(grant(first)); await admit(visit);
  expect(reads).toHaveLength(2); // Cached admission and same-reader renewal never poll Meta.
  expect(transport.send.mock.calls.filter(([request]) => new URL(request.url).pathname.endsWith('/auth/refresh'))).toHaveLength(1);
});

it('suppresses a late former caller permission and lets only the current identity refresh it', async () => {
  client.adopt(grant(first));
  const admission = observed(admit(visit)); await read(1);
  const shell = observeFrame();
  const oldFreshness = refreshMetaForSignIn(); freshness.push(oldFreshness);
  refreshOnSignIn(); client.adopt(grant(second));
  await answer(1, meta({ may_create_org: true }));
  expect(await admission).toEqual({ ok: true });
  const current = await read(2);
  expect(current.request.headers.get('authorization')).toBe('Bearer synthetic-second');
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false);
  await answer(2, meta({ may_create_org: false })); await Promise.all(freshness);
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false);
  expect(reads).toHaveLength(2); shell(); expect(observers()).toBe(0);
});

it('retains the authorized followup when cached anonymous Meta settles before restoration and Shell is not ready', async () => {
  const admission = observed(admit(visit)); await read(1);
  const detach = observeFrame(); refreshOnSignIn();
  await answer(1, meta({ may_create_org: false }));
  expect(observers()).toBe(2); // Meta is cached, but admission still owns its restoration boundary.
  await restoreAs(); expect(await admission).toEqual({ ok: true });
  const authorized = await read(2);
  expect(authorized.request.headers.get('authorization')).toBe('Bearer synthetic-first');
  expect(observers()).toBe(2); // AuthFrame and the current caller's refresh own this flight.
  detach();
  expect(observers()).toBe(1); expect(authorized.request.signal.aborted).toBe(false);
  await answer(2, meta({ may_create_org: true })); await Promise.all(freshness);
  expect(observers()).toBe(0);
  const shell = observeFrame();
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(true);
  expect(reads).toHaveLength(2); shell();
});

it('cannot restore a former caller permission when explicit cancellation reverts an already-running cached refetch', async () => {
  client.adopt(grant(first));
  queryClient.setQueryData(metaKey, meta({ may_create_org: true }));
  const detach = observeFrame();
  const oldRefetch = observed(queryClient.refetchQueries({ queryKey: metaKey, exact: true }, { cancelRefetch: false }));
  const old = await read(1);
  refreshOnSignIn(); client.adopt(grant(second));
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false);
  await queryClient.cancelQueries({ queryKey: metaKey, exact: true });
  await oldRefetch; await Promise.all(freshness);
  expect(old.request.signal.aborted).toBe(true);
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false);
  expect(reads).toHaveLength(1); detach();
  // The next actual reader may make its one fresh read; the canceled work never retries itself.
  const shell = observeFrame(), current = await read(2);
  expect(current.request.headers.get('authorization')).toBe('Bearer synthetic-second');
  old.reply.resolve(Response.json(meta({ may_create_org: true })));
  await answer(2, meta({ may_create_org: false }));
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false);
  expect(reads).toHaveLength(2); shell();
});

it.each(['cancel', 'sign out'] as const)('preserves explicit cancellation despite the admission lease (%s)', async (boundary) => {
  client.adopt(grant(first));
  const admission = observed(admit(visit)), initial = await read(1);
  queryClient.setQueryData(['private-old-reader'], 'Old private cache');
  const oldFreshness = refreshMetaForSignIn(); freshness.push(oldFreshness);
  expect(observers()).toBe(2);
  if (boundary === 'cancel') await queryClient.cancelQueries({ queryKey: metaKey, exact: true });
  else {
    // The real app's sign-out listener uses this same explicit cache clear, independent of observers.
    release.push(client.onSession((event) => { if (event.type === 'signed_out') queryClient.clear(); }));
    await client.signOut();
    expect(queryClient.getQueryData(['private-old-reader'])).toBeUndefined();
  }
  const result = await admission;
  expect(result.ok).toBe(false);
  if (result.ok) throw new Error('Explicit cancellation was swallowed.');
  expect(result.error).toBeInstanceOf(CancelledError); expect(initial.request.signal.aborted).toBe(true);
  await oldFreshness; expect(reads).toHaveLength(1); expect(observers()).toBe(0);
  client.adopt(grant(second));
  const next = observed(admit(visit)); await read(2);
  // A late200 from the retired request cannot replace the new reader's cache.
  initial.reply.resolve(Response.json(meta({ may_create_org: true })));
  await answer(2, meta({ may_create_org: false })); expect(await next).toEqual({ ok: true });
  expect(queryClient.getQueryData<Meta>(metaKey)?.may_create_org).toBe(false); expect(reads).toHaveLength(2);
});

it('releases the admission observer and keeps a real Meta refusal as that same error', async () => {
  const admission = observed(admit(visit)); const initial = await read(1), detach = observeFrame(); refreshOnSignIn(detach);
  await restoreAs();
  initial.reply.resolve(Response.json({ type: 'orch:error:internal', code: 'internal', title: 'Failed', detail: '', status: 500, request_id: 'meta-read' }, { status: 500 }));
  const result = await admission;
  expect(result.ok).toBe(false);
  if (result.ok) throw new Error('The Meta refusal was swallowed.');
  expect(result.error).toBeInstanceOf(ApiError);
  await Promise.all(freshness); expect(reads).toHaveLength(1); expect(observers()).toBe(0);
  expect(queryClient.getQueryState(metaKey)?.error).toBe(result.error);
});

it.each([false, true])('releases admission when restoration fails while Meta is pending, preserving another legitimate observer (%s)', async (keepFrame) => {
  const admission = observed(admit(visit)), initial = await read(1);
  const detach = keepFrame ? observeFrame() : undefined;
  await restoreStarted.promise;
  restoreReply.resolve(Response.json({ type: 'orch:error:internal', code: 'internal', title: 'Failed', detail: '', status: 500, request_id: 'restore-read' }, { status: 500 }));
  const result = await admission;
  expect(result.ok).toBe(false);
  if (result.ok) throw new Error('The restoration refusal was swallowed.');
  expect(result.error).toBeInstanceOf(ApiError);
  expect(observers()).toBe(keepFrame ? 1 : 0);
  expect(initial.request.signal.aborted).toBe(!keepFrame);
  if (keepFrame) {
    await answer(1, meta({ name: 'Still owned by AuthFrame', may_create_org: false }));
    expect(queryClient.getQueryData<Meta>(metaKey)?.name).toBe('Still owned by AuthFrame');
    detach?.();
  } else initial.reply.resolve(Response.json(meta({ may_create_org: false })));
  expect(observers()).toBe(0); expect(reads).toHaveLength(1);
});

it('keeps the native setup gate and releases its retained reader before redirecting', async () => {
  const admission = observed(admit(visit)); await read(1); await restoreAs();
  await answer(1, meta({ needs_setup: true, may_create_org: false }));
  const result = await admission;
  expect(result.ok).toBe(false);
  if (result.ok || !isRedirect(result.error)) throw new Error('Setup must still redirect.');
  expect(result.error.options.to).toBe('/setup'); expect(observers()).toBe(0); expect(reads).toHaveLength(1);
});
