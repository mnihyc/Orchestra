/** The real socket/follower/store path carries immutable selectors and never acknowledges a skipped branch cut. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { queryClient } from '@/api/query';
import { createRealtimeSocket, type RealtimeSocket } from '@/realtime/socket';
import { applyThreadFeedRows, createThreadReadFeed } from '@/realtime/threadFeed';
import { EVICT_AFTER_MS, UNSUBSCRIBE_AFTER_MS, FeedRegistry, followFeed, type FeedFollower } from '@/realtime/sync';
import { setThreadReadChangeHandler, setThreadReadRetainer, threadViewOf, threadWriter, flushDeltas } from '@/realtime/stores/thread';
import { A, B, C, Signals, Wire, answer, deferred, opening, projection, snapshot, viewedPrompt } from './continuation-fixtures';

const stopped: (() => void)[] = [];

function transport(handler: (request: Request) => Promise<Response>) {
  const calls: Request[] = [];
  const requests = new Signals<Request>();
  vi.stubGlobal('fetch', (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input, init);
    calls.push(request); requests.emit(request); return handler(request);
  });
  return { calls, requests };
}

function connection() {
  const wires: Wire[] = [];
  const socket = createRealtimeSocket({ url: () => 'ws://synthetic.test/api/v1/ws', token: () => 'synthetic-token',
    renew: () => Promise.resolve(true), onSuspended: () => undefined, random: () => 0.5,
    connect: () => { const wire = new Wire(); wires.push(wire); return wire; } });
  socket.start();
  const wire = wires[0]; if (wire === undefined) throw new Error('No test connection');
  wire.ready(); stopped.push(() => socket.stop());
  return { socket, wire, wires };
}

function follow(socket: RealtimeSocket, id: string): FeedFollower {
  const follower = followFeed(socket, createThreadReadFeed(id, { rows: (rows, reset) => { applyThreadFeedRows(id, rows, reset); } }));
  setThreadReadChangeHandler((changed) => { if (changed === id) follower.reload(); });
  stopped.push(() => follower.stop());
  return follower;
}

function ack(wire: Wire, sub: { id: number; feed: string }, scope = projection()): void {
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, mode: 'delta', seq: 10, projection: scope });
}

beforeEach(() => {
  vi.useFakeTimers(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined);
});
afterEach(() => {
  for (const stop of stopped.splice(0).reverse()) stop();
  flushDeltas(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined);
  vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
});

it('sends captured explicit selectors on initial subscribe and reconnect while normal subscriptions remain unchanged', async () => {
  const { socket, wire, wires } = connection();
  const received = vi.fn();
  const listener = { continuationId: B, projectionRevision: 7, since: () => 10, onFrame: received };
  socket.subscribe('thread:synthetic', listener);
  const first = await wire.subscriptions.next();
  expect(first).toMatchObject({ continuation_id: B, projection_revision: 7, since: 10 });
  // The caller changing its object does not rewrite this subscription's captured selector.
  listener.continuationId = C; listener.projectionRevision = 8;
  socket.subscribe('org:synthetic', { since: () => null, onFrame: () => undefined });
  const plain = await wire.subscriptions.next();
  expect(plain).not.toHaveProperty('continuation_id'); expect(plain).not.toHaveProperty('projection_revision');
  wire.onclose?.({ code: 1006 });
  await vi.advanceTimersByTimeAsync(500);
  const reconnect = wires[1];
  if (reconnect === undefined) throw new Error('Reconnect was not opened');
  reconnect.ready();
  const again = await reconnect.subscriptions.next();
  const againPlain = await reconnect.subscriptions.next();
  expect(again).toMatchObject({ continuation_id: B, projection_revision: 7, since: 10 });
  expect(again.id).not.toBe(first.id);
  expect(againPlain).not.toHaveProperty('continuation_id');
  wire.say({ op: 'subscribed', id: first.id, feed: first.feed, seq: 10, mode: 'delta' });
  expect(received).not.toHaveBeenCalled();
  reconnect.say({ op: 'subscribed', id: again.id, feed: again.feed, seq: 10, mode: 'delta' });
  expect(received).toHaveBeenCalledTimes(1);
});

it('loads once, follows active without an explicit id, and makes no idle/per-prompt reads', async () => {
  const sync = { ...snapshot(), items: [answer('a', 'Active')] }; const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); follow(socket, id);
  const sub = await wire.subscriptions.next();
  expect(sub).not.toHaveProperty('continuation_id'); expect(sub.projection_revision).toBe(1);
  ack(wire, sub);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(http.calls).toHaveLength(1);
  expect(threadViewOf(id).order).toEqual(['a']);
  expect(http.calls.every((call) => !new URL(call.url).pathname.endsWith('/versions'))).toBe(true);
});

it('clears and reloads a changed follow-active projection even when its item sequence moves backward', async () => {
  let sync = { ...snapshot(), items: [answer('a', 'Active A')] };
  const id = sync.thread.id; const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const first = await wire.subscriptions.next(); ack(wire, first);
  sync = { ...snapshot(projection(B, B, 2), 5), items: [answer('b', 'Active B', B, 5)] };
  wire.say({ op: 'sync', id: first.id, feed: first.feed, prev: 10, seq: 11,
    data: { projection: projection(B, B, 2), items: [answer('wrong', 'Must not merge', B)] } });
  expect(threadViewOf(id).items.size).toBe(0);
  const next = await wire.subscriptions.next(); ack(wire, next, projection(B, B, 2));
  expect(http.calls).toHaveLength(2); expect(follower.lastSeq()).toBe(5);
  expect(threadViewOf(id).order).toEqual(['b']);
  wire.say({ op: 'delta', id: first.id, feed: first.feed, item_id: 'b', field: 'text', offset: 8, len: 4, data: ' old' });
  flushDeltas();
  const held = threadViewOf(id).items.get('b');
  expect(held?.kind === 'agent_message' ? held.payload.text : '').toBe('Active B');
});

it('discards complete:false without advancing its cursor and then resubscribes with the fresh opening identity', async () => {
  let sync = { ...snapshot(), items: [answer('a', 'Before')] }; const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const first = await wire.subscriptions.next(); ack(wire, first);
  sync = { ...snapshot(projection(), 20), items: [answer('fresh', 'Complete replacement')] };
  wire.say({ op: 'sync', id: first.id, feed: first.feed, prev: 10, seq: 19,
    data: { projection: projection(), complete: false, items: [answer('partial', 'Do not acknowledge')] } });
  expect(follower.lastSeq()).toBeNull();
  const next = await wire.subscriptions.next(); ack(wire, next);
  expect(follower.lastSeq()).toBe(20); expect(next.since).toBe(20);
  expect(threadViewOf(id).items.has('partial')).toBe(false);
  expect(threadViewOf(id).order).toEqual(['fresh']); expect(http.calls).toHaveLength(2);
});

it('keeps an explicit projection while active selection and viewed status progress change without another read', async () => {
  const scope = projection(B, A, 7, false); const sync = { ...snapshot(scope), ...{ viewed_prompt: viewedPrompt('preparing') } };
  const id = sync.thread.id; selectThreadContinuation(id, B);
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const sub = await wire.subscriptions.next(); ack(wire, sub, scope);
  const previous = threadViewOf(id).read;
  const delivered = deferred<undefined>();
  const watch = () => {
    if (threadViewOf(id).viewedPrompt?.status === 'failed') delivered.resolve(undefined);
  };
  const { useThreadStore } = await import('@/realtime/stores/thread');
  const unsubscribe = useThreadStore.subscribe(watch); stopped.push(unsubscribe);
  wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: 10, seq: 11, data: {
    projection: { ...scope, active_continuation_id: C, execution_revision: 9 }, viewed_prompt: viewedPrompt('failed'),
  } });
  await delivered.promise;
  expect(threadViewOf(id).read?.epoch).toBe(previous?.epoch);
  expect(threadViewOf(id).read?.projection?.active_continuation_id).toBe(C);
  expect(follower.lastSeq()).toBe(11); expect(http.calls).toHaveLength(1);
});

it('retains HTTP-only scope in the existing registry without loading/subscribing and evicts it with that cache', async () => {
  const registry = new FeedRegistry();
  setThreadReadRetainer((id) => registry.retain(`thread:${id}`, () => threadWriter.evict(id)));
  const id = snapshot().thread.id;
  const captured = captureThreadRead(id);
  const http = transport(() => Promise.reject(new Error('No transport is allowed for cache retention')));
  expect(threadViewOf(id).read?.lifetime).toBe(captured.scope.lifetime);
  await vi.advanceTimersByTimeAsync(EVICT_AFTER_MS);
  expect(threadViewOf(id).read).toBeNull(); expect(http.calls).toHaveLength(0);
  const next = captureThreadRead(id); expect(next.scope.identity).not.toBe(captured.scope.identity);
  registry.clear();
});

it('validates the subscribed projection before accepting any old-source cursor or delta', async () => {
  let sync = { ...snapshot(), items: [answer('a', 'Old projection')] }; const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const first = await wire.subscriptions.next();
  sync = { ...snapshot(projection(B, B, 2), 12), items: [answer('b', 'New projection', B)] };
  ack(wire, first, projection(B, B, 2));
  expect(follower.lastSeq()).toBeNull();
  const next = await wire.subscriptions.next();
  expect(next.projection_revision).toBe(2); expect(next.since).toBe(12);
  ack(wire, next, projection(B, B, 2));
  expect(threadViewOf(id).order).toEqual(['b']); expect(http.calls).toHaveLength(2);
});

it('repairs a real sequence gap with a complete bounded opening and renews the captured subscription', async () => {
  const initial = { ...snapshot(), items: [answer('a', 'Initial')] }; const id = initial.thread.id;
  let repair = false;
  const http = transport((request) => {
    if (new URL(request.url).pathname.endsWith('/sync')) return Promise.resolve(Response.json({ ...snapshot(), complete: false, since: 10 }));
    return Promise.resolve(Response.json(opening(repair ? { ...snapshot(projection(), 20), items: [answer('fresh', 'Fresh')] } : initial)));
  });
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const first = await wire.subscriptions.next(); ack(wire, first); repair = true;
  wire.say({ op: 'sync', id: first.id, feed: first.feed, prev: 14, seq: 15,
    data: { projection: projection(), items: [answer('skipped', 'Do not apply')] } });
  const next = await wire.subscriptions.next(); ack(wire, next);
  expect(next.since).toBe(20); expect(follower.lastSeq()).toBe(20);
  expect(threadViewOf(id).order).toEqual(['fresh']); expect(http.calls).toHaveLength(3);
  const delivered = deferred<undefined>();
  const { useThreadStore } = await import('@/realtime/stores/thread');
  const unsubscribe = useThreadStore.subscribe(() => { if (threadViewOf(id).items.has('later')) delivered.resolve(undefined); });
  stopped.push(unsubscribe);
  wire.say({ op: 'sync', id: next.id, feed: next.feed, prev: 20, seq: 21,
    data: { projection: projection(), items: [answer('later', 'Incremental again', A, 21)] } });
  await delivered.promise;
  expect(threadViewOf(id).order).toEqual(['fresh','later']); expect(follower.lastSeq()).toBe(21);
});

it('retires an in-flight positioned read when authority closes the current subscription', async () => {
  const initial = snapshot(); const id = initial.thread.id; const pending = deferred<Response>();
  const http = transport((request) => new URL(request.url).pathname.endsWith('/opening')
    ? Promise.resolve(Response.json(opening(initial))) : pending.promise);
  const { socket, wire } = connection(); follow(socket, id);
  const sub = await wire.subscriptions.next(); ack(wire, sub);
  await http.requests.next();
  const { fetchItem } = await import('@/api/threads');
  const old = fetchItem(id, 'private-item'); await http.requests.next();
  const refusal = expect(old).rejects.toMatchObject({ name: 'AbortError' });
  wire.say({ op: 'closed', id: sub.id, feed: sub.feed, reason: 'revoked' });
  pending.resolve(Response.json(answer('private-item', 'Must not publish'))); await refusal;
  expect(threadViewOf(id).closed).toBe('revoked'); expect(threadViewOf(id).items.has('private-item')).toBe(false);
});

it('cannot advance a modern cursor past an incomplete native Thread object or spin a repair loop', async () => {
  const sync = { ...snapshot(), items: [answer('held', 'Held source')] }; const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const first = await wire.subscriptions.next(); ack(wire, first);
  const partial = { ...sync.thread }; Reflect.deleteProperty(partial, 'can');
  wire.say({ op: 'sync', id: first.id, feed: first.feed, prev: 10, seq: 11,
    data: { projection: projection(), thread: partial } });
  expect(follower.lastSeq()).toBeNull();
  expect(threadViewOf(id).thread).toHaveProperty('can.export');
  expect(threadViewOf(id).order).toEqual(['held']); expect(threadViewOf(id).error).not.toBeNull();
  await vi.advanceTimersByTimeAsync(60_000);
  expect(http.calls).toHaveLength(1);
});

it('a late old-view release cannot evict or stop a replacement cache entry after reader clear', async () => {
  const registry = new FeedRegistry();
  const oldStop = vi.fn(), oldEvict = vi.fn(), nextStop = vi.fn(), nextEvict = vi.fn();
  const original: FeedFollower = { lastSeq: () => 10, reload: () => undefined, refresh: () => undefined, stop: oldStop };
  const replacement: FeedFollower = { lastSeq: () => 20, reload: () => undefined, refresh: () => undefined, stop: nextStop };
  const releaseOld = registry.use('thread:shared', () => original, oldEvict);
  const releaseOldRead = registry.hold('thread:shared', oldEvict);
  registry.clear();
  const releaseNext = registry.use('thread:shared', () => replacement, nextEvict);
  releaseOld(); releaseOldRead();
  await vi.advanceTimersByTimeAsync(UNSUBSCRIBE_AFTER_MS + EVICT_AFTER_MS);
  expect(oldStop).toHaveBeenCalledTimes(1); expect(oldEvict).toHaveBeenCalledTimes(1);
  expect(nextStop).not.toHaveBeenCalled(); expect(nextEvict).not.toHaveBeenCalled();
  releaseNext(); await vi.advanceTimersByTimeAsync(UNSUBSCRIBE_AFTER_MS + EVICT_AFTER_MS);
  expect(nextStop).toHaveBeenCalledTimes(1); expect(nextEvict).toHaveBeenCalledTimes(1);
});

it('visible nonstreaming and pending HTTP owners prevent cache eviction without opening another subscription', async () => {
  const registry = new FeedRegistry(); const evict = vi.fn();
  registry.retain('thread:reader', evict);
  const releaseVisible = registry.hold('thread:reader', evict);
  const releaseBody = registry.hold('thread:reader', evict);
  await vi.advanceTimersByTimeAsync(EVICT_AFTER_MS * 2); expect(evict).not.toHaveBeenCalled();
  releaseVisible(); await vi.advanceTimersByTimeAsync(EVICT_AFTER_MS); expect(evict).not.toHaveBeenCalled();
  releaseBody(); await vi.advanceTimersByTimeAsync(EVICT_AFTER_MS); expect(evict).toHaveBeenCalledTimes(1);
  registry.clear();
});

it('a missing modern subscribed identity is a truthful refusal, not an endless HTTP opening loop', async () => {
  const sync = snapshot(); const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket, id);
  const sub = await wire.subscriptions.next();
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, seq: 10, mode: 'delta' });
  expect(follower.lastSeq()).toBeNull(); expect(threadViewOf(id).error).not.toBeNull();
  await vi.advanceTimersByTimeAsync(60_000); expect(http.calls).toHaveLength(1);
});


it('refuses another explicit selected history instead of repeatedly reloading the requested history', async () => {
  const scope = projection(B,A,2,false); const sync = { ...snapshot(scope), items: [answer('kept','Selected B',B)] };
  const id = sync.thread.id; selectThreadContinuation(id,B);
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket,id);
  const sub = await wire.subscriptions.next();
  ack(wire,sub,projection(C,A,2,false));
  expect(follower.lastSeq()).toBeNull(); expect(threadViewOf(id).error).not.toBeNull();
  expect(threadViewOf(id).read?.selection).toBe(B); expect(threadViewOf(id).order).toEqual(['kept']);
  await vi.advanceTimersByTimeAsync(60_000); expect(http.calls).toHaveLength(1);
});


it('retires a socket row already queued for application before a selection switch', async () => {
  let sync = { ...snapshot(), items: [answer('a','A')] }; const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); follow(socket,id);
  const first = await wire.subscriptions.next(); ack(wire,first);
  wire.say({ op: 'sync', id: first.id, feed: first.feed, prev: 10, seq: 11,
    data: { projection: projection(), items: [answer('queued-old','Never B',A,11)] } });
  sync = { ...snapshot(projection(B,A,2,false)), items: [answer('b','B',B)] };
  selectThreadContinuation(id,B);
  const next = await wire.subscriptions.next(); ack(wire,next,projection(B,A,2,false));
  expect(threadViewOf(id).read?.selection).toBe(B);
  expect(threadViewOf(id).order).toEqual(['b']); expect(threadViewOf(id).items.has('queued-old')).toBe(false);
  expect(http.calls).toHaveLength(2);
});

it('keeps the default load/subscribe/delta observer working when older peers omit every optional continuation field', async () => {
  const sync = { ...snapshot(null), items: [answer('legacy','Hello')] }; const id = sync.thread.id;
  const http = transport(() => Promise.resolve(Response.json(opening(sync))));
  const { socket, wire } = connection(); const follower = follow(socket,id);
  const sub = await wire.subscriptions.next();
  expect(sub).not.toHaveProperty('continuation_id'); expect(sub).not.toHaveProperty('projection_revision');
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, seq: 10, mode: 'delta' });
  wire.say({ op: 'delta', id: sub.id, feed: sub.feed, item_id: 'legacy', field: 'text', offset: 5, len: 1, data: '!' });
  flushDeltas();
  const item = threadViewOf(id).items.get('legacy');
  expect(item?.kind === 'agent_message' ? item.payload.text : '').toBe('Hello!');
  expect(threadViewOf(id).closed).toBeNull(); expect(threadViewOf(id).error).toBeNull();
  expect(follower.lastSeq()).toBe(10); expect(http.calls).toHaveLength(1);
});
