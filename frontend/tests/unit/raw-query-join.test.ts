/** Actual observers + the one socket, with synthetic HTTP barriers; no native execution is claimed. */
import { CancelledError, InfiniteQueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client, ApiError } from '@/api/client';
import { queryClient } from '@/api/query';
import { viewedRawFramesQuery } from '@/api/threadHistory';
import { captureThreadRead, isCurrentThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { fetchThreadSync, threadKeys } from '@/api/threads';
import type { Draft, RawEvent, ThreadContinuationProjection, ThreadOpening, ThreadParameters, ThreadRawListReply } from '@/generated/api-types';
import { installQueryFeeds, queryFeed } from '@/realtime/queryFeeds';
import { RawSourceError } from '@/realtime/rawQuery';
import { createRealtimeSocket, type ClientFrame } from '@/realtime/socket';
import { contentThreadRead, defaultThreadRead, resultThreadRead, sameThreadRead, threadReadRegistryKey, threadReceipt, type ThreadReadRef } from '@/realtime/threadScope';
import { holdThreadRead, setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer, threadViewOf, threadWriter, useThreadStore } from '@/realtime/stores/thread';
import { A, B, C, deferred, opening as nativeOpening, projection, Signals, snapshot, Wire } from './continuation-fixtures';
import { coordinator, ownedRows, serve, T } from './thread-read-fixtures';
import { coordinateThreadReads } from '@/realtime/threadFeed';
import { FeedRegistry } from '@/realtime/sync';

const AT = '2026-10-17T00:00:00Z';
const route = defaultThreadRead(T), left = resultThreadRead(T,'raw-left'), right = resultThreadRead(T,'raw-right');
const cleanups: (() => void)[] = [];
type Sub = Extract<ClientFrame, { op: 'sub' }>;
type Result = ReturnType<ReturnType<typeof observe>['observer']['getCurrentResult']>;
interface Pending { readonly request: Request; readonly resolve: (response: Response) => void }
const event = (n: number, text = `raw-${n}`): RawEvent => ({ n, at: AT, direction: 'from_harness', payload: { text } });
const scopeOf = (ref: ThreadReadRef): ThreadContinuationProjection => {
  const scope = captureThreadRead(ref).scope.projection;
  if (scope === null) throw new Error('The real opening must supply a projection');
  return scope;
};
function rawServer(extra?: (request: Request) => Promise<Response> | undefined) {
  const reads: Pending[] = [], issued = new Signals<Pending>();
  const server = serve((request) => {
    if (!new URL(request.url).pathname.endsWith('/raw')) return extra?.(request);
    const pending = deferred<Response>();
    const reading = { request, resolve: pending.resolve };
    reads.push(reading); issued.emit(reading);
    return pending.promise;
  });
  return { ...server, reads, issued };
}
async function open(ref: ThreadReadRef, selection: string | null) {
  selectThreadContinuation(ref, selection); await fetchThreadSync(ref, null);
}
function reply(pending: Pending, ref: ThreadReadRef, events: RawEvent[], older = false,
  scope = scopeOf(ref)): void {
  pending.resolve(Response.json({ events, has_older: older, projection: scope }));
}
function transport() {
  const wires: Wire[] = [];
  const socket = createRealtimeSocket({ url: () => 'ws://orch.test/api/v1/ws', token: () => 'synthetic-raw-reader',
    renew: () => Promise.resolve(true), onSuspended: () => undefined, random: () => 0.5,
    connect: () => { const wire = new Wire(); wires.push(wire); return wire; } });
  const stop = installQueryFeeds(socket, queryClient);
  cleanups.push(() => { stop(); socket.stop(); });
  socket.start();
  const first = wires[0]; if (first === undefined) throw new Error('Expected one physical socket'); first.ready();
  const last = () => { const wire = wires.at(-1); if (wire === undefined) throw new Error('Expected the socket'); return wire; };
  return { socket, wires, wire: first, last, stop };
}
interface PublicationBarrier {
  afterPage?: (page: ThreadRawListReply) => Promise<ThreadRawListReply>;
  beforeCommit?: () => void;
}
function observationOptions(ref: ThreadReadRef, segment: string, barrier?: PublicationBarrier): ReturnType<typeof viewedRawFramesQuery> {
  const options = viewedRawFramesQuery(ref, segment);
  if (barrier === undefined) return options;
  const read = options.queryFn, sharing = options.structuralSharing;
  if (typeof read !== 'function' || typeof sharing !== 'function') throw new Error('Expected the actual scoped raw query and publication guard');
  return { ...options,
    queryFn: async (context) => {
      const page = await read(context);
      return barrier.afterPage === undefined ? page : barrier.afterPage(page);
    },
    structuralSharing: (previous, next) => { barrier.beforeCommit?.(); return sharing(previous, next); },
  };
}
function observe(ref: ThreadReadRef, segment = 'segment-one', follow = false, barrier?: PublicationBarrier) {
  let options = observationOptions(ref, segment, barrier);
  const observer = new InfiniteQueryObserver(queryClient, options);
  const off = observer.subscribe(() => undefined);
  let key = JSON.stringify(options.queryKey);
  // The real UI's inherited-scope hook rebuilds pure options after a verified owner transition.
  const unwatch = follow ? useThreadStore.subscribe(() => {
    options = observationOptions(ref, segment, barrier);
    const next = JSON.stringify(options.queryKey);
    if (key !== next) { key = next; observer.setOptions(options); }
  }) : () => undefined;
  const close = () => { unwatch(); off(); };
  cleanups.push(close);
  return { observer, close, key: () => options.queryKey };
}
function until(view: ReturnType<typeof observe>, predicate: (result: Result) => boolean): Promise<Result> {
  const now = view.observer.getCurrentResult();
  if (predicate(now)) return Promise.resolve(now);
  return new Promise((resolve) => {
    let off: () => void = () => undefined;
    const inspect = (result: Result) => { if (predicate(result)) { off(); resolve(result); } };
    off = view.observer.subscribe(inspect); inspect(view.observer.getCurrentResult());
  });
}
const settled = (view: ReturnType<typeof observe>) => until(view, (result) => !result.isFetching && (result.isSuccess || result.isError));
function ack(wire: Wire, sub: Sub, scope: ThreadContinuationProjection, after = 0, mode = 'ephemeral') {
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, seq: null, mode, projection: scope,
    raw: { segment_id: sub.raw_segment_id, after } });
}
function live(wire: Wire, sub: Sub, scope: ThreadContinuationProjection, value: RawEvent,
  segment = sub.raw_segment_id) {
  wire.say({ op: 'event', id: sub.id, feed: sub.feed, data: { segment_id: segment, projection: scope, event: value } });
}
function numbers(view: ReturnType<typeof observe>): number[] {
  return view.observer.getCurrentResult().data?.pages[0]?.events.map((row) => row.n) ?? [];
}
const subscriptions = (wire: Wire) => wire.sent.filter((frame): frame is Sub => frame.op === 'sub');

beforeEach(() => {
  vi.useFakeTimers(); queryClient.clear(); threadWriter.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});
afterEach(() => {
  for (const close of cleanups.splice(0).reverse()) close();
  threadWriter.clear(); queryClient.clear(); vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});

it('ACK gates the actual initial GET, stages live rows and commits valid infinite data without losing the newer row', async () => {
  const server = rawServer(); await open(left, B); const viewBefore = threadViewOf(left);
  const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  expect(sub).toMatchObject({ feed: `thread:${T}:raw`, raw_segment_id: 'segment-one', continuation_id: B, projection_revision: 2 });
  expect('since' in sub).toBe(false); expect(server.reads).toHaveLength(0);
  expect(queryClient.getQueryData(view.key())).toBeUndefined();
  const scope = scopeOf(left); ack(wire, sub, scope, 10);
  const read = await server.issued.next();
  expect(new URL(read.request.url).searchParams.get('limit')).toBe('100');
  live(wire, sub, scope, event(12)); live(wire, sub, scope, event(11));
  expect(queryClient.getQueryData(view.key())).toBeUndefined();
  reply(read, left, [event(10)]); await settled(view);
  expect(numbers(view)).toEqual([12, 11, 10]);
  expect(view.observer.getCurrentResult().data?.pageParams).toEqual([null]);
  expect(threadViewOf(left)).toBe(viewBefore);
});

it('coalesces equal-source observers and does not let an unmount cancel the remaining observer', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport();
  const one = observe(left), two = observe(left), sub = await wire.subscriptions.next();
  expect(subscriptions(wire)).toHaveLength(1); ack(wire, sub, scopeOf(left));
  const read = await server.issued.next(); expect(server.reads).toHaveLength(1);
  one.close(); await vi.advanceTimersByTimeAsync(0);
  expect(read.request.signal.aborted).toBe(false);
  expect(wire.sent.filter((frame) => frame.op === 'unsub')).toHaveLength(0);
  reply(read, left, [event(1)]); await settled(two); expect(numbers(two)).toEqual([1]);
  two.close(); await vi.advanceTimersByTimeAsync(0);
  expect(wire.sent.filter((frame) => frame.op === 'unsub')).toHaveLength(1);
});

it('coalesces native sources across owners while keeping their cancellation and query windows independent', async () => {
  const server = rawServer(); await open(left, B); await open(right, B); const { wire } = transport();
  const one = observe(left), two = observe(right), sub = await wire.subscriptions.next(); ack(wire, sub, scopeOf(left));
  const first = await server.issued.next(), second = await server.issued.next();
  expect(subscriptions(wire)).toHaveLength(1); expect(one.key()).not.toEqual(two.key());
  one.close(); await vi.advanceTimersByTimeAsync(0);
  expect(first.request.signal.aborted).toBe(true); expect(second.request.signal.aborted).toBe(false);
  live(wire, sub, scopeOf(right), event(2, 'right still owns this source'));
  reply(second, right, [event(1)]); await settled(two);
  reply(first, left, [event(99, 'late closed owner')]); await vi.advanceTimersByTimeAsync(0);
  expect(numbers(two)).toEqual([2, 1]); expect(server.reads).toHaveLength(2);
});

it('nine same-thread readers never mix segment-local numbers and keep local owner keys off the wire', async () => {
  const server = rawServer(); const owners = Array.from({ length: 9 }, (_, index) => resultThreadRead(T, `raw-owner-${index}`));
  for (const owner of owners) await open(owner, B);
  const { wire, wires } = transport();
  const views = owners.map((owner, index) => observe(owner, `segment-${index}`));
  const subs: Sub[] = [];
  for (const owner of owners) { const sub = await wire.subscriptions.next(); subs.push(sub); ack(wire, sub, scopeOf(owner)); }
  const reads: Pending[] = [];
  for (const _owner of owners) reads.push(await server.issued.next());
  for (let index = 0; index < owners.length; index += 1) {
    const owner = owners[index], sub = subs[index], read = reads[index];
    if (owner === undefined || sub === undefined || read === undefined) throw new Error('Expected every real observer source');
    reply(read, owner, [event(1, `only-${index}`)]);
    live(wire, sub, scopeOf(owner), event(2, `live-${index}`));
    live(wire, sub, scopeOf(owner), event(500, 'wrong-segment'), 'another-segment');
  }
  await Promise.all(views.map(settled));
  for (let index = 0; index < views.length; index += 1) {
    const view = views[index]; if (view === undefined) throw new Error('Expected the view');
    expect(view.observer.getCurrentResult().data?.pages[0]?.events).toEqual([event(2, `live-${index}`), event(1, `only-${index}`)]);
  }
  expect(wires).toHaveLength(1); expect(subscriptions(wire)).toHaveLength(9);
  expect(JSON.stringify(subs)).not.toContain('raw-owner-');
});

it('delayed old publications at or below the admission floor cannot enter a newcomer', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left);
  const sub = await wire.subscriptions.next(), scope = scopeOf(left); ack(wire, sub, scope, 50);
  const read = await server.issued.next();
  live(wire, sub, scope, event(49, 'old publication')); live(wire, sub, scope, event(50, 'old admission floor'));
  live(wire, sub, scope, event(51)); reply(read, left, [event(50)]); await settled(view);
  expect(numbers(view)).toEqual([51, 50]);
  expect(view.observer.getCurrentResult().data?.pages[0]?.events[1]?.payload).toEqual({ text: 'raw-50' });
});

it('older-page reads retain the exact subscription and late pages cannot erase a newer head', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left);
  const sub = await wire.subscriptions.next(), scope = scopeOf(left); ack(wire, sub, scope);
  reply(await server.issued.next(), left, [event(4), event(3)], true); await settled(view);
  const fetching = view.observer.fetchNextPage(), older = await server.issued.next();
  expect(new URL(older.request.url).searchParams.get('before')).toBe('3');
  live(wire, sub, scope, event(5));
  reply(older, left, [event(2), event(1)]); await fetching; await vi.advanceTimersByTimeAsync(0);
  expect(numbers(view)).toEqual([5, 4, 3]);
  expect(view.observer.getCurrentResult().data?.pages[1]?.events.map((row) => row.n)).toEqual([2, 1]);
  expect(subscriptions(wire)).toHaveLength(1); expect(server.reads).toHaveLength(2);
});

it('reconnect ACK performs one head read and leaves the gap before loaded older pages explicitly pageable', async () => {
  const server = rawServer(); await open(left, B); const rig = transport(), view = observe(left);
  const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub, scopeOf(left));
  reply(await server.issued.next(), left, [event(6), event(5)], true); await settled(view);
  const fetching = view.observer.fetchNextPage(); reply(await server.issued.next(), left, [event(4), event(3)], true); await fetching;
  rig.wire.readyState = 3; rig.wire.onclose?.({ code: 1006 }); await vi.advanceTimersByTimeAsync(500);
  const reconnected = rig.last(); expect(reconnected).not.toBe(rig.wire); reconnected.ready();
  const next = await reconnected.subscriptions.next();
  expect(server.reads).toHaveLength(2); ack(reconnected, next, scopeOf(left), 10);
  const repair = await server.issued.next(); expect(new URL(repair.request.url).searchParams.has('before')).toBe(false);
  reply(repair, left, [event(10), event(9)], true);
  await until(view, () => numbers(view)[0] === 10);
  expect(view.observer.getCurrentResult().data?.pages[1]?.events.map((row) => row.n)).toEqual([4, 3]);
  const filling = view.observer.fetchNextPage(), gap = await server.issued.next();
  expect(new URL(gap.request.url).searchParams.get('before')).toBe('9'); reply(gap, left, [event(8), event(7), event(6), event(5)], true); await filling;
  expect(server.reads).toHaveLength(4); expect(subscriptions(reconnected)).toHaveLength(1);
});

it('a permission-only resync rejoins this source without reopening its owner or any unrelated thread', async () => {
  const server = rawServer(); await open(route, null); await open(left, B); const routeBefore = threadViewOf(route), ownerBefore = threadViewOf(left);
  const { wire } = transport(), view = observe(left); const sub = await wire.subscriptions.next(); ack(wire, sub, scopeOf(left));
  reply(await server.issued.next(), left, [event(1)]); await settled(view);
  const count = server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening')).length;
  wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reason: 'same_feed' });
  const fresh = await wire.subscriptions.next(); expect(fresh.id).not.toBe(sub.id); expect(server.reads).toHaveLength(1);
  ack(wire, fresh, scopeOf(left), 2); reply(await server.issued.next(), left, [event(2), event(1)]);
  await until(view, () => numbers(view)[0] === 2);
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(count);
  expect(threadViewOf(route)).toBe(routeBefore); expect(threadViewOf(left)).toBe(ownerBefore);
});

it('a changed-projection reload opens only its actual owner before any new raw GET', async () => {
  const joined = coordinator(); cleanups.push(() => joined.registry.clear()); const server = rawServer();
  await open(route, null); await open(left, B); const old = captureThreadRead(left), routeBefore = threadViewOf(route);
  const { wire } = transport(), view = observe(left, 'segment-one', true), sub = await wire.subscriptions.next();
  server.state.selectedRevision = 3;
  ack(wire, sub, { ...scopeOf(left), projection_revision: 3 }, 0, 'reload');
  await vi.advanceTimersByTimeAsync(0);
  expect(isCurrentThreadRead(old)).toBe(false); expect(threadViewOf(left).read?.selection).toBe(B);
  expect(threadViewOf(left).read?.projection?.projection_revision).toBe(3);
  expect(threadViewOf(route)).toBe(routeBefore); expect(server.reads).toHaveLength(0);
  const fresh = await wire.subscriptions.next(); expect(fresh.projection_revision).toBe(3); expect(fresh.continuation_id).toBe(B);
  // The retired ACK/error cannot restart old revision 2 or fail the fresh owner.
  ack(wire, sub, scopeOf(left)); wire.say({ op: 'error', id: sub.id, code: 'not_found', detail: 'retired' });
  ack(wire, fresh, scopeOf(left)); reply(await server.issued.next(), left, [event(7)]); await settled(view);
  expect(numbers(view)).toEqual([7]);
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))
    .map((request) => new URL(request.url).searchParams.get('continuation_id'))).toEqual([null, B, B]);
});

it('same-projection reload is a source error, with neither a raw GET nor an owner-opening loop', async () => {
  const joined = coordinator(); cleanups.push(() => joined.registry.clear()); const server = rawServer(); await open(left, B);
  const owner = threadViewOf(left), { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  ack(wire, sub, scopeOf(left), 0, 'reload');
  const result = await settled(view); expect(result.error).toBeInstanceOf(RawSourceError);
  expect(result.error).toMatchObject({ code: 'bad_feed' }); expect(server.reads).toHaveLength(0);
  expect(threadViewOf(left)).toBe(owner); await vi.advanceTimersByTimeAsync(60_000);
  expect(subscriptions(wire)).toHaveLength(1); expect(server.calls).toHaveLength(1);
});

it('read_unavailable keeps the original-history refusal and never guesses an origin or treats events as a page', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  wire.say({ op: 'error', id: sub.id, code: 'read_unavailable', detail: 'Open its original history to read these whole-session frames.' });
  const result = await settled(view);
  expect(result.error).toMatchObject({ code: 'read_unavailable', message: 'Open its original history to read these whole-session frames.' });
  expect('continuation_id' in (result.error ?? {})).toBe(false); expect(result.data).toBeUndefined(); expect(server.reads).toHaveLength(0);
  live(wire, sub, scopeOf(left), event(1, 'must not become data')); await vi.advanceTimersByTimeAsync(0);
  expect(view.observer.getCurrentResult().data).toBeUndefined(); expect(view.observer.getCurrentResult().error).toBe(result.error);
});

it.each([403, 404, 500])('HTTP %i stays an error and cannot be replaced by staged or later events', async (status) => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  ack(wire, sub, scopeOf(left)); const read = await server.issued.next(); live(wire, sub, scopeOf(left), event(2));
  const code = status === 403 ? 'authz.denied' : status === 404 ? 'not_found' : 'internal';
  read.resolve(Response.json({ code, status, title: 'Synthetic raw refusal', detail: '', request_id: 'synthetic' }, { status }));
  const failed = await settled(view); expect(failed.error).toBeInstanceOf(ApiError); expect(failed.error).toMatchObject({ status });
  live(wire, sub, scopeOf(left), event(3)); await vi.advanceTimersByTimeAsync(0);
  expect(view.observer.getCurrentResult().error).toBe(failed.error); expect(view.observer.getCurrentResult().data).toBeUndefined();
  expect(server.reads).toHaveLength(1);
});

it('away-back retirement rejects old HTTP, ACKs and errors without borrowing the new owner epoch', async () => {
  const server = rawServer(); await open(route, null); await open(left, B); const routeBefore = threadViewOf(route);
  const rig = transport(), view = observe(left, 'segment-one', true), sub = await rig.wire.subscriptions.next(), oldScope = scopeOf(left);
  ack(rig.wire, sub, oldScope); const oldRead = await server.issued.next(), oldCapture = captureThreadRead(left);
  await open(left, C); await open(left, B); await vi.advanceTimersByTimeAsync(0);
  // A C subscription can be created and retired during the two real openings. Only B's new live id is used.
  const fresh = subscriptions(rig.wire).at(-1); if (fresh === undefined) throw new Error('Expected current B source');
  expect(fresh.id).not.toBe(sub.id); expect(oldRead.request.signal.aborted).toBe(true); expect(isCurrentThreadRead(oldCapture)).toBe(false);
  ack(rig.wire, fresh, scopeOf(left)); reply(await server.issued.next(), left, [event(8)]); await settled(view);
  oldRead.resolve(Response.json({ projection: oldScope, events: [event(99, 'old epoch')], has_older: false }));
  ack(rig.wire, sub, oldScope); rig.wire.say({ op: 'error', id: sub.id, code: 'forbidden', detail: 'old epoch' });
  await vi.advanceTimersByTimeAsync(0); expect(numbers(view)).toEqual([8]); expect(threadViewOf(route)).toBe(routeBefore);
});

it('staging overflow retires the old attempt and reanchors only this source with the unchanged 100-row page budget', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  const scope = scopeOf(left); ack(wire, sub, scope); const first = await server.issued.next();
  for (let n = 2; n <= 102; n += 1) live(wire, sub, scope, event(n));
  const fresh = await wire.subscriptions.next(); expect(first.request.signal.aborted).toBe(true);
  expect(queryClient.getQueryData(view.key())).toBeUndefined(); expect(server.reads).toHaveLength(1);
  ack(wire, fresh, scope, 102); const repair = await server.issued.next();
  expect(new URL(repair.request.url).searchParams.get('limit')).toBe('100');
  const head = Array.from({ length: 100 }, (_, index) => event(102 - index)); reply(repair, left, head, true);
  await settled(view); first.resolve(Response.json({ projection: scope, events: [event(1)], has_older: false }));
  await vi.advanceTimersByTimeAsync(0); expect(numbers(view)).toEqual(head.map((row) => row.n));
  expect(view.observer.getCurrentResult().hasNextPage).toBe(true); expect(server.reads).toHaveLength(2);
});

it('current permission closure clears only its admitted source and late frames cannot revive it', async () => {
  const server = rawServer(); await open(left, B); await open(right, C); const { wire } = transport();
  const one = observe(left), two = observe(right); const first = await wire.subscriptions.next(), second = await wire.subscriptions.next();
  ack(wire, first, scopeOf(left)); ack(wire, second, scopeOf(right));
  reply(await server.issued.next(), left, [event(1, 'B')]); reply(await server.issued.next(), right, [event(1, 'C')]);
  await Promise.all([settled(one), settled(two)]);
  wire.say({ op: 'closed', id: first.id, feed: first.feed, reason: 'revoked' });
  live(wire, first, scopeOf(left), event(2, 'no longer authorized')); live(wire, second, scopeOf(right), event(2, 'C remains'));
  await vi.advanceTimersByTimeAsync(0);
  expect(one.observer.getCurrentResult()).toMatchObject({ status: 'error', data: undefined });
  expect(numbers(two)).toEqual([2, 1]); expect(two.observer.getCurrentResult().error).toBeNull();
});

it('factories and cold/disabled owners create no raw demand, and the generic bridge cannot seed their state', async () => {
  const server = rawServer(); const { wire } = transport(); const cold = resultThreadRead(T,'cold-raw-window');
  const before = threadViewOf(cold), options = viewedRawFramesQuery(cold, 'segment-one');
  expect(options.enabled).toBe(false); expect(threadViewOf(cold)).toBe(before);
  observe(cold); await vi.advanceTimersByTimeAsync(0);
  expect(subscriptions(wire)).toHaveLength(0); expect(server.calls).toHaveLength(0);
  expect(queryFeed(options.queryKey)).toBeNull(); expect(queryClient.getQueryData(options.queryKey)).toBeUndefined();
});

it('a new browser reader cannot receive a prior reader raw HTTP result or retain its native subscription', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  ack(wire, sub, scopeOf(left)); const read = await server.issued.next(), original = scopeOf(left), reader = client.session.getState().readerVersion;
  try {
    client.session.setState({ readerVersion: (reader ?? 0) + 1 }); await vi.advanceTimersByTimeAsync(0);
    read.resolve(Response.json({ projection: original, events: [event(1, 'old reader')], has_older: false }));
    live(wire, sub, original, event(2, 'old reader event')); await vi.advanceTimersByTimeAsync(0);
    expect(view.observer.getCurrentResult().data).toBeUndefined(); expect(wire.sent.filter((frame) => frame.op === 'unsub')).toHaveLength(1);
    expect(server.reads).toHaveLength(1);
  } finally { client.session.setState({ readerVersion: reader }); }
});


it('a refused normal owner opening remains failed, never loops the old raw revision or changes a different owner', async () => {
  let refuse = false;
  const server = rawServer((request) => refuse && new URL(request.url).pathname.endsWith('/opening')
    ? Promise.resolve(Response.json({ code: 'authz.denied', status: 403, title: 'Synthetic current refusal', detail: '', request_id: 'synthetic' }, { status: 403 })) : undefined);
  const joined = coordinator(); cleanups.push(() => joined.registry.clear());
  await open(route, null); await open(left, B); const routeBefore = threadViewOf(route);
  const { wire } = transport(), view = observe(left, 'segment-one', true), sub = await wire.subscriptions.next();
  refuse = true; server.state.selectedRevision = 3;
  ack(wire, sub, { ...scopeOf(left), projection_revision: 3 }, 0, 'reload');
  await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(left).error).toBeInstanceOf(ApiError); expect(threadViewOf(left).read?.ready).toBe(false);
  expect(threadViewOf(route)).toBe(routeBefore); expect(server.reads).toHaveLength(0);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(subscriptions(wire)).toHaveLength(1); expect(server.calls).toHaveLength(3);
  expect(view.observer.getCurrentResult().data).toBeUndefined();
});

it('wrong-feed and wrong-source frames do not admit or populate an observer before its exact ACK', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  const scope = scopeOf(left);
  wire.say({ op: 'subscribed', id: sub.id, feed: 'thread:another-thread:raw', mode: 'ephemeral', seq: null,
    projection: scope, raw: { segment_id: sub.raw_segment_id, after: 0 } });
  live(wire, sub, scope, event(1, 'before ACK'));
  expect(server.reads).toHaveLength(0); expect(view.observer.getCurrentResult().data).toBeUndefined();
  ack(wire, sub, scope); const read = await server.issued.next();
  live(wire, sub, { ...scope, projection_revision: 99 }, event(9, 'another projection'));
  live(wire, sub, scope, event(10, 'another segment'), 'not-this-segment');
  reply(read, left, [event(1)]); await settled(view); expect(numbers(view)).toEqual([1]);
});


it('a concurrent older-page completion never publishes its captured head over a newer reanchor', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), scope = scopeOf(left);
  const sub = await wire.subscriptions.next(); ack(wire, sub, scope);
  reply(await server.issued.next(), left, [event(6), event(5)], true); await settled(view);
  wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reason: 'same_feed' });
  const fresh = await wire.subscriptions.next(); ack(wire, fresh, scope, 10);
  const head = await server.issued.next();
  const olderFetching = view.observer.fetchNextPage(), older = await server.issued.next();
  reply(head, left, [event(10), event(9)], true); await until(view, () => numbers(view)[0] === 10);
  const observed: number[] = [], unlisten = view.observer.subscribe((result) => {
    const n = result.data?.pages[0]?.events[0]?.n; if (n !== undefined) observed.push(n);
  });
  live(wire, fresh, scope, event(11));
  reply(older, left, [event(4), event(3)], true); await olderFetching; await vi.advanceTimersByTimeAsync(0); unlisten();
  expect(numbers(view)).toEqual([11, 10, 9]); expect(observed.every((n) => n >= 10)).toBe(true);
  expect(view.observer.getCurrentResult().data?.pages[1]?.events.map((row) => row.n)).toEqual([4, 3]);
  expect(server.reads).toHaveLength(3); expect(subscriptions(wire)).toHaveLength(2);
});


it('removing an observed query retires its listener instead of allowing a late event to recreate the cache entry', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  const scope = scopeOf(left); ack(wire, sub, scope); const pending = await server.issued.next();
  queryClient.removeQueries({ queryKey: view.key(), exact: true });
  live(wire, sub, scope, event(99, 'removed query')); reply(pending, left, [event(1)]);
  await vi.advanceTimersByTimeAsync(0);
  expect(queryClient.getQueryData(view.key())).toBeUndefined();
  expect(wire.sent.filter((frame) => frame.op === 'unsub')).toHaveLength(1);
  const retrying = view.observer.refetch(), fresh = await wire.subscriptions.next();
  expect(fresh.id).not.toBe(sub.id); ack(wire, fresh, scope); reply(await server.issued.next(), left, [event(2)]); await retrying;
  expect(numbers(view)).toEqual([2]); expect(server.reads).toHaveLength(2);
});


it('an explicit retry cannot be overwritten by an old refused attempt whose HTTP error arrives late in the same owner epoch', async () => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left), scope = scopeOf(left);
  const sub = await wire.subscriptions.next(); ack(wire, sub, scope); const old = await server.issued.next();
  wire.say({ op: 'error', id: sub.id, code: 'forbidden', detail: 'Synthetic admission revoked' });
  await settled(view);
  const retrying = view.observer.refetch(), fresh = await wire.subscriptions.next(); ack(wire, fresh, scope);
  reply(await server.issued.next(), left, [event(5)]); await retrying;
  old.resolve(Response.json({ code: 'internal', status: 500, title: 'Old refused attempt', detail: '', request_id: 'synthetic' }, { status: 500 }));
  await vi.advanceTimersByTimeAsync(0);
  expect(numbers(view)).toEqual([5]); expect(view.observer.getCurrentResult().error).toBeNull();
  expect(server.reads).toHaveLength(2); expect(captureThreadRead(left).scope.projection).toEqual(scope);
});


it('an explicit original source ignores unrelated active-execution metadata without reopening its owner', async () => {
  const server = rawServer(); await open(left, B); const original = threadViewOf(left), { wire } = transport(), view = observe(left);
  const sub = await wire.subscriptions.next(), latest = { ...scopeOf(left), active_continuation_id: C, execution_revision: 20 };
  ack(wire, sub, latest); const read = await server.issued.next();
  reply(read, left, [event(1)], false, latest); await settled(view);
  live(wire, sub, latest, event(2)); await vi.advanceTimersByTimeAsync(0);
  expect(numbers(view)).toEqual([2, 1]); expect(threadViewOf(left)).toBe(original);
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(1);
});

it('follow-active ABA reloads its own verified cut while preserving omitted continuation_id and an unrelated owner', async () => {
  const joined = coordinator(); cleanups.push(() => joined.registry.clear()); const server = rawServer();
  await open(route, null); await open(right, null); const before = threadViewOf(route), old = captureThreadRead(right);
  const { wire } = transport(), view = observe(right, 'segment-one', true), sub = await wire.subscriptions.next();
  server.state.revision = 3;
  ack(wire, sub, { ...scopeOf(right), projection_revision: 3, execution_revision: 3 }, 0, 'reload');
  await vi.advanceTimersByTimeAsync(0);
  expect(isCurrentThreadRead(old)).toBe(false); expect(threadViewOf(route)).toBe(before); expect(server.reads).toHaveLength(0);
  const fresh = await wire.subscriptions.next(); expect(fresh.projection_revision).toBe(3); expect('continuation_id' in fresh).toBe(false);
  ack(wire, fresh, scopeOf(right)); const read = await server.issued.next();
  expect(new URL(read.request.url).searchParams.has('continuation_id')).toBe(false);
  expect(new URL(read.request.url).searchParams.get('projection_revision')).toBe('3');
  reply(read, right, [event(3)]); await settled(view); expect(numbers(view)).toEqual([3]);
});


it('coalesced refusal errors retain separate captured owners for any later explicit original-history refusal read', async () => {
  const server = rawServer(); await open(left, B); await open(right, B); const { wire } = transport();
  const one = observe(left), two = observe(right), sub = await wire.subscriptions.next();
  wire.say({ op: 'error', id: sub.id, code: 'read_unavailable', detail: 'Whole original history required' });
  const results = await Promise.all([settled(one), settled(two)]);
  const first = results[0].error, second = results[1].error;
  if (first === null || second === null) throw new Error('Expected both actual refusal errors');
  const firstCapture = threadReceipt(first), secondCapture = threadReceipt(second);
  if (firstCapture === undefined || secondCapture === undefined) throw new Error('Expected source-stamped refusal receipts');
  expect(sameThreadRead(firstCapture.ref, left)).toBe(true); expect(sameThreadRead(secondCapture.ref, right)).toBe(true);
  expect(first).not.toBe(second); expect(server.reads).toHaveLength(0);
});


// These follow-up cases run the original queryFn, observer and socket. Barriers only hold its
// accepted return or retire ownership at the real structural-sharing boundary.
it.each(['selection', 'reader', 'manager stop', 'query removal', 'attempt replacement'] as const)(
  'post-queryFn publication remains fenced after %s and actual manager reconciliation', async (phase) => {
    const server = rawServer(); await open(route, null); await open(left, B); const other = threadViewOf(route);
    const rig = transport(), accepted = deferred<ThreadRawListReply>(), release = deferred<undefined>();
    const view = observe(left, 'segment-one', false, { afterPage: async (page) => {
      accepted.resolve(page); await release.promise; return page;
    } });
    const delivered: number[][] = [], untrace = view.observer.subscribe((result) => {
      if (result.isSuccess) delivered.push(result.data.pages[0]?.events.map((row) => row.n) ?? []);
    }); cleanups.push(untrace);
    const key = view.key(), sub = await rig.wire.subscriptions.next(), scope = scopeOf(left);
    ack(rig.wire, sub, scope); reply(await server.issued.next(), left, [event(1, 'accepted but not published')]);
    const page = await accepted.promise; expect(page.events[0]?.n).toBe(1);
    expect(queryClient.getQueryData(key)).toBeUndefined();
    const reader = client.session.getState().readerVersion;
    try {
      if (phase === 'selection') selectThreadContinuation(left, C);
      else if (phase === 'reader') client.session.setState({ readerVersion: (reader ?? 0) + 1 });
      else if (phase === 'manager stop') rig.stop();
      else if (phase === 'query removal') queryClient.removeQueries({ queryKey: key, exact: true });
      else rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reason: 'same_feed' });
      await vi.advanceTimersByTimeAsync(0);
      release.resolve(undefined); await vi.advanceTimersByTimeAsync(0);
      expect(delivered).toEqual([]); expect(queryClient.getQueryData(key)).toBeUndefined();
      expect(view.observer.getCurrentResult().data).toBeUndefined();
      expect(threadViewOf(route)).toBe(other); expect(server.reads).toHaveLength(1);
      expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(2);
      if (phase === 'attempt replacement' || phase === 'manager stop') {
        // Same owner/reader/epoch, different admission: equality of the reader alone is insufficient.
        expect(scopeOf(left)).toEqual(scope);
        if (phase === 'manager stop') cleanups.push(installQueryFeeds(rig.socket, queryClient));
        const fresh = await rig.wire.subscriptions.next(); ack(rig.wire, fresh, scope, 2);
        reply(await server.issued.next(), left, [event(2, 'current admitted head')]);
        await until(view, () => numbers(view)[0] === 2);
        expect(delivered).toEqual([[2]]); expect(server.reads).toHaveLength(2);
      }
    } finally { rig.stop(); client.session.setState({ readerVersion: reader }); }
  });

it.each(['selection', 'manager stop', 'query removal', 'attempt replacement'] as const)(
  'the final cache guard rejects %s even after the Query retryer has already resolved', async (phase) => {
    const server = rawServer(); await open(route, null); await open(left, B); const other = threadViewOf(route);
    const rig = transport(); let retire: () => void = () => undefined, once = false;
    const view = observe(left, 'segment-one', false, { beforeCommit: () => {
      if (!once) { once = true; retire(); }
    } });
    const heads: number[][] = [], untrace = view.observer.subscribe((result) => {
      if (result.isSuccess) heads.push(result.data.pages[0]?.events.map((row) => row.n) ?? []);
    }); cleanups.push(untrace);
    const sub = await rig.wire.subscriptions.next(), scope = scopeOf(left), key = view.key();
    retire = () => {
      if (phase === 'selection') selectThreadContinuation(left, C);
      else if (phase === 'manager stop') rig.stop();
      else if (phase === 'query removal') queryClient.removeQueries({ queryKey: key, exact: true });
      else rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reason: 'same_feed' });
    };
    ack(rig.wire, sub, scope); reply(await server.issued.next(), left, [event(9, 'must fail final publication')]);
    await vi.advanceTimersByTimeAsync(0);
    expect(once).toBe(true); expect(heads).toEqual([]); expect(queryClient.getQueryData(key)).toBeUndefined();
    expect(view.observer.getCurrentResult().error).toBeNull(); expect(threadViewOf(route)).toBe(other);
    if (phase === 'attempt replacement') {
      const fresh = await rig.wire.subscriptions.next(); ack(rig.wire, fresh, scope, 10);
      reply(await server.issued.next(), left, [event(10, 'fresh publication')]);
      await until(view, () => numbers(view)[0] === 10); expect(heads).toEqual([[10]]);
    }
  });

it.each([
  { name: 'H1 newer snapshot before later-started H2 older snapshot', first: 'H1', live: true, top: 100 },
  { name: 'H2 older snapshot before H1 newer snapshot', first: 'H2', live: true, top: 100 },
  { name: 'known H1 head survives H2 without any live row', first: 'H1', live: false, top: 100 },
  { name: 'three heads exceeding the row bound keep real older coverage', first: 'H1', live: true, top: 130 },
] as const)('three-head ordering: $name', async ({ first, live: stream, top }) => {
  const server = rawServer(); await open(left, B); const before = threadViewOf(left), rig = transport(), view = observe(left);
  const sub = await rig.wire.subscriptions.next(), scope = scopeOf(left);
  ack(rig.wire, sub, scope); reply(await server.issued.next(), left, Array.from({ length: 80 }, (_, i) => event(80 - i)));
  await settled(view);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reason: 'same_feed' });
  const fresh = await rig.wire.subscriptions.next(); ack(rig.wire, fresh, scope, 80);
  const h1 = await server.issued.next();
  const refetching = view.observer.refetch(), h2 = await server.issued.next();
  if (stream) for (let n = 81; n <= top; n += 1) live(rig.wire, fresh, scope, event(n));
  const newer = Array.from({ length: 100 }, (_, i) => event(top - i));
  const olderTop = top === 100 ? 90 : 120;
  const older = Array.from({ length: Math.min(olderTop, 100) }, (_, i) => event(olderTop - i));
  const observed: number[][] = [], untrace = view.observer.subscribe((result) => {
    if (result.data !== undefined) observed.push(result.data.pages[0]?.events.map((row) => row.n) ?? []);
  }); cleanups.push(untrace);
  reply(first === 'H1' ? h1 : h2, left, first === 'H1' ? newer : older, top > 100);
  await until(view, () => numbers(view)[0] === top);
  const published = observed.findIndex((rows) => rows[0] === top); expect(published).toBeGreaterThanOrEqual(0);
  reply(first === 'H1' ? h2 : h1, left, first === 'H1' ? older : newer, top > 100);
  await refetching; await vi.advanceTimersByTimeAsync(0);
  expect(numbers(view)).toEqual(newer.map((row) => row.n));
  expect(observed.slice(published).every((rows) => rows.length === 100 && rows[0] === top)).toBe(true);
  expect(view.observer.getCurrentResult().data?.pages[0]?.has_older).toBe(top > 100);
  expect(server.reads).toHaveLength(3); expect(subscriptions(rig.wire)).toHaveLength(2); expect(rig.wires).toHaveLength(1);
  expect(server.reads.every((read) => new URL(read.request.url).searchParams.get('limit') === '100')).toBe(true);
  expect(threadViewOf(left)).toBe(before); expect(scopeOf(left)).toEqual(scope);
});

function trackedCoordinator() {
  const registry = new FeedRegistry(), holds = new Map<string, number>(), originalHold = registry.hold.bind(registry);
  vi.spyOn(registry, 'hold').mockImplementation((key, evict, rebase) => {
    const release = originalHold(key, evict, rebase); holds.set(key, (holds.get(key) ?? 0) + 1); let closed = false;
    return () => { if (!closed) { closed = true; holds.set(key, (holds.get(key) ?? 0) - 1); release(); } };
  });
  coordinateThreadReads(registry, (_id, ref) => ownedRows(ref)); cleanups.push(() => registry.clear());
  return { holds, registry };
}
function nativeCut(scope: ThreadContinuationProjection): ThreadOpening {
  const rows = snapshot(scope, 20);
  return nativeOpening({ ...rows, thread: { ...rows.thread, active_continuation_id: scope.active_continuation_id, execution_revision: scope.execution_revision } });
}

it('removing the final raw observer during its held normal opening releases ownership without resurrection', async () => {
  const joined = trackedCoordinator(), opened = new Signals<Request>(), pending = deferred<Response>(); let hold = false;
  const server = rawServer((request) => {
    if (!hold || !new URL(request.url).pathname.endsWith('/opening')) return undefined;
    opened.emit(request); return pending.promise;
  });
  await open(route, null); await open(left, B); const other = threadViewOf(route);
  const rig = transport(), view = observe(left, 'segment-one', true), sub = await rig.wire.subscriptions.next();
  hold = true; server.state.selectedRevision = 3;
  const next = { ...scopeOf(left), projection_revision: 3 };
  ack(rig.wire, sub, next, 0, 'reload');
  await opened.next(); await vi.advanceTimersByTimeAsync(0);
  const owner = threadReadRegistryKey(left);
  // Real normal HTTP read plus the raw transition's visible-owner hold, not a fake wake.
  expect(joined.holds.get(owner)).toBe(2); expect(server.reads).toHaveLength(0);
  view.close(); await vi.advanceTimersByTimeAsync(0); expect(joined.holds.get(owner)).toBe(1);
  pending.resolve(Response.json(nativeCut(next))); await vi.advanceTimersByTimeAsync(0);
  expect(joined.holds.get(owner)).toBe(0); expect(subscriptions(rig.wire)).toHaveLength(1); expect(server.reads).toHaveLength(0);
  expect(threadViewOf(route)).toBe(other);
  const count = server.calls.length; await vi.advanceTimersByTimeAsync(60_000); expect(server.calls).toHaveLength(count);
});

it('contentThreadRead preserves controls:true through verified active recovery while fixed and historical reads keep shared controls and drafts', async () => {
  const joined = coordinator(); cleanups.push(() => joined.registry.clear());
  const content = contentThreadRead(T, 'raw-full-content'), fixed = resultThreadRead(T, 'raw-fixed-preview');
  let version = 1, hold = false;
  const entered = new Signals<Request>(), release = deferred<Response>();
  const parameters = (): ThreadParameters => ({ approval_mode: 'ask', approval_reviewer: null, parameters: [],
    effort: { source: { scope: 'default', node: null }, supported: true, value: null, writable: false },
    model: { source: { scope: 'default', node: null }, options: [], value: `synthetic-model-${version}`, writable: false } });
  const body = (scope: ThreadContinuationProjection): ThreadOpening => ({ ...nativeCut(scope), parameters: parameters(),
    limits: { bypass_all: version === 3, reached: [] }, draft: null });
  const server = rawServer((request) => {
    const url = new URL(request.url); if (!url.pathname.endsWith('/opening')) return undefined;
    const selected = url.searchParams.get('continuation_id');
    const scope = { ...projection(selected ?? server.state.active, server.state.active, selected === null ? server.state.revision : server.state.selectedRevision, selected === null), execution_revision: server.state.revision };
    if (hold) { entered.emit(request); return release.promise; }
    return Promise.resolve(Response.json(body(scope)));
  });
  await open(route, null);
  const draft: Draft = { content: [{ type: 'text', text: 'Unsent main draft' }], delivery: 'queue', native_action: null, review_comments: [], updated_at: AT };
  queryClient.setQueryData(threadKeys.draft(T), draft); const dirty = queryClient.getQueryData(threadKeys.draft(T));
  version = 2; await open(content, null);
  // The full page, like useThreadReadScope/the normal follower, remains a visible owner.
  const releaseContent = holdThreadRead(content); cleanups.push(releaseContent);
  expect(queryClient.getQueryData<ThreadParameters>(threadKeys.parameters(T))?.model.value).toBe('synthetic-model-2');
  await open(fixed, B); const shared = queryClient.getQueryData(threadKeys.parameters(T)), other = threadViewOf(route);
  const rig = transport(), fixedView = observe(fixed, 'segment-fixed', true), fixedSub = await rig.wire.subscriptions.next();
  ack(rig.wire, fixedSub, scopeOf(fixed)); reply(await server.issued.next(), fixed, [event(11, 'fixed original bytes')]); await settled(fixedView);
  const fixedData = fixedView.observer.getCurrentResult().data;
  const view = observe(content, 'segment-one', true), sub = await rig.wire.subscriptions.next();
  expect('continuation_id' in sub).toBe(false); expect('controls' in sub).toBe(false);
  version = 3; hold = true; server.state.revision = 3;
  const current = { ...scopeOf(content), projection_revision: 3, execution_revision: 3 };
  ack(rig.wire, sub, current, 0, 'reload'); await entered.next();
  expect(queryClient.getQueryData(threadKeys.parameters(T))).toBe(shared); expect(server.reads).toHaveLength(1);
  release.resolve(Response.json(body(current))); await vi.advanceTimersByTimeAsync(0);
  expect(captureThreadRead(content).ref.controls).toBe(true); expect(threadViewOf(content).read?.selection).toBeNull();
  expect(queryClient.getQueryData<ThreadParameters>(threadKeys.parameters(T))?.model.value).toBe('synthetic-model-3');
  expect(queryClient.getQueryData(threadKeys.draft(T))).toBe(dirty); expect(threadViewOf(route)).toBe(other);
  const fresh = await rig.wire.subscriptions.next(); ack(rig.wire, fresh, scopeOf(content));
  reply(await server.issued.next(), content, [event(3)]); await settled(view);
  expect(fixedView.observer.getCurrentResult().data).toBe(fixedData);
  live(rig.wire, fixedSub, { ...scopeOf(fixed), execution_revision: 3 }, event(12, 'fixed still observes only itself'));
  await vi.advanceTimersByTimeAsync(0); expect(numbers(fixedView)).toEqual([12, 11]);
  const afterActive = queryClient.getQueryData(threadKeys.parameters(T)); hold = false; version = 4;
  selectThreadContinuation(content, B); await vi.advanceTimersByTimeAsync(0);
  expect(queryClient.getQueryData(threadKeys.parameters(T))).toBe(afterActive); expect(queryClient.getQueryData(threadKeys.draft(T))).toBe(dirty);
  expect(threadViewOf(fixed).read?.selection).toBe(B); expect(threadViewOf(route)).toBe(other);
  expect(server.reads).toHaveLength(2); expect(A).toBe(current.viewed_continuation_id);
});

it('an authority reload immediately retires every raw page and late result while retaining its exact owner and ACK gate', async () => {
  const server = rawServer(); await open(left, B); await open(right, C);
  const leftOwner = threadViewOf(left), rightOwner = threadViewOf(right);
  const { wire } = transport(), view = observe(left), other = observe(right);
  const sub = await wire.subscriptions.next(), otherSub = await wire.subscriptions.next();
  const scope = scopeOf(left); ack(wire, sub, scope); ack(wire, otherSub, scopeOf(right));
  reply(await server.issued.next(), left, [event(6), event(5)], true);
  reply(await server.issued.next(), right, [event(1, 'unrelated')]);
  await Promise.all([settled(view), settled(other)]);
  const older = view.observer.fetchNextPage();
  reply(await server.issued.next(), left, [event(4), event(3)], true); await older;
  const retired = view.observer.getCurrentResult().data;
  if (retired?.pages[1] === undefined) throw new Error('Expected a published older page');
  const oldTail = retired.pages[1];
  const fetching = view.observer.fetchNextPage(), held = await server.issued.next();
  const ownedQuery = queryClient.getQueryCache().find({ queryKey: view.key(), exact: true });
  const reads = server.reads.length;
  wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  expect(view.observer.getCurrentResult().data).toBeUndefined();
  expect(queryClient.getQueryData(view.key())).toBeUndefined();
  expect(queryClient.getQueryCache().find({ queryKey: view.key(), exact: true })).toBe(ownedQuery);
  expect(held.request.signal.aborted).toBe(true);
  expect(numbers(other)).toEqual([1]);
  expect(threadViewOf(left)).toBe(leftOwner); expect(threadViewOf(right)).toBe(rightOwner);
  const fresh = await wire.subscriptions.next();
  expect(fresh).toMatchObject({ feed: sub.feed, raw_segment_id: sub.raw_segment_id, continuation_id: B, projection_revision: 2 });
  expect(fresh.id).not.toBe(sub.id); expect(server.reads).toHaveLength(reads);
  ack(wire, sub, scope); live(wire, sub, scope, event(99, 'retired wire'));
  held.resolve(Response.json({ events: [event(2, 'retired HTTP')], has_older: false, projection: scope }));
  await fetching; await vi.advanceTimersByTimeAsync(0);
  expect(view.observer.getCurrentResult().data).toBeUndefined(); expect(server.reads).toHaveLength(reads);
  ack(wire, fresh, scope, 7);
  const repair = await server.issued.next();
  expect(new URL(repair.request.url).searchParams.has('before')).toBe(false);
  reply(repair, left, [event(7, 'readmitted')]); await until(view, () => numbers(view)[0] === 7);
  const current = view.observer.getCurrentResult().data;
  if (current === undefined) throw new Error('Expected the readmitted raw page');
  expect(current.pages).toHaveLength(1); expect(current.pageParams).toEqual([null]);
  expect(() => queryClient.setQueryData(view.key(), {
    ...current, pages: [...current.pages, oldTail], pageParams: [...current.pageParams, 5],
  })).toThrow(CancelledError);
  expect(view.observer.getCurrentResult().data).toBe(current);
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(2);
});

it.each([undefined, false])('an ordinary raw gap with reload=%s preserves admitted pages until its exact fresh ACK repairs the head', async (reload) => {
  const server = rawServer(); await open(left, B); const { wire } = transport(), view = observe(left);
  const sub = await wire.subscriptions.next(), scope = scopeOf(left); ack(wire, sub, scope);
  reply(await server.issued.next(), left, [event(6), event(5)], true); await settled(view);
  const fetching = view.observer.fetchNextPage();
  reply(await server.issued.next(), left, [event(4), event(3)], true); await fetching;
  const admitted = view.observer.getCurrentResult().data;
  wire.say({ op: 'resync', id: sub.id, feed: sub.feed, ...(reload === undefined ? {} : { reload }) });
  expect(view.observer.getCurrentResult().data).toBe(admitted);
  const fresh = await wire.subscriptions.next(); expect(server.reads).toHaveLength(2);
  ack(wire, fresh, scope, 8); const repair = await server.issued.next();
  expect(view.observer.getCurrentResult().data).toBe(admitted);
  reply(repair, left, [event(8), event(7)], true); await until(view, () => numbers(view)[0] === 8);
  expect(view.observer.getCurrentResult().data?.pages[1]).toBe(admitted?.pages[1]);
});

it('authority reload clears both coalesced raw owners and closing one keeps the other ACK-gated owner alive', async () => {
  const server = rawServer(); await open(left, B); await open(right, B); const { wire } = transport();
  const one = observe(left), two = observe(right), sub = await wire.subscriptions.next();
  ack(wire, sub, scopeOf(left));
  reply(await server.issued.next(), left, [event(1, 'left')]);
  reply(await server.issued.next(), right, [event(1, 'right')]);
  await Promise.all([settled(one), settled(two)]);
  wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  expect(one.observer.getCurrentResult().data).toBeUndefined();
  expect(two.observer.getCurrentResult().data).toBeUndefined();
  const fresh = await wire.subscriptions.next(); one.close(); await vi.advanceTimersByTimeAsync(0);
  expect(subscriptions(wire)).toHaveLength(2); expect(server.reads).toHaveLength(2);
  ack(wire, fresh, scopeOf(right)); reply(await server.issued.next(), right, [event(2)]);
  await until(two, () => numbers(two)[0] === 2);
  expect(two.observer.getCurrentResult().data?.pages).toHaveLength(1);
  expect(server.reads).toHaveLength(3);
});

it('hiding and remounting the same raw query preserves admitted tails while fencing the retired consumers pending page', async () => {
  const server = rawServer(); await open(left, B); const owner = threadViewOf(left);
  const { wire } = transport(), view = observe(left), sub = await wire.subscriptions.next();
  const scope = scopeOf(left); ack(wire, sub, scope);
  reply(await server.issued.next(), left, [event(6), event(5)], true); await settled(view);
  const older = view.observer.fetchNextPage();
  reply(await server.issued.next(), left, [event(4), event(3)], true); await older;
  const admitted = view.observer.getCurrentResult().data;
  const query = queryClient.getQueryCache().find({ queryKey: view.key(), exact: true });
  const fetching = view.observer.fetchNextPage(), retiredRead = await server.issued.next();
  view.close(); await vi.advanceTimersByTimeAsync(0);
  expect(retiredRead.request.signal.aborted).toBe(true);
  expect(wire.sent.filter((frame) => frame.op === 'unsub')).toHaveLength(1);
  expect(queryClient.getQueryData(view.key())).toBe(admitted);
  expect(threadViewOf(left)).toBe(owner);
  const remounted = observe(left), fresh = await wire.subscriptions.next();
  expect(queryClient.getQueryCache().find({ queryKey: remounted.key(), exact: true })).toBe(query);
  expect(remounted.observer.getCurrentResult().data).toBe(admitted);
  expect(fresh.id).not.toBe(sub.id); expect(server.reads).toHaveLength(3);
  retiredRead.resolve(Response.json({ events: [event(2, 'retired consumer')], has_older: false, projection: scope }));
  await fetching; await vi.advanceTimersByTimeAsync(0);
  expect(remounted.observer.getCurrentResult().data).toBe(admitted);
  ack(wire, fresh, scope, 8); reply(await server.issued.next(), left, [event(8), event(7)], true);
  await until(remounted, (result) => result.error !== null || result.data?.pages[0]?.events[0]?.n === 8);
  expect(remounted.observer.getCurrentResult().error).toBeNull();
  expect(numbers(remounted)).toEqual([8, 7]);
  expect(remounted.observer.getCurrentResult().data?.pages[1]).toBe(admitted?.pages[1]);
  expect(remounted.observer.getCurrentResult().data?.pageParams).toEqual([null, 5]);
  expect(threadViewOf(left)).toBe(owner);
});
