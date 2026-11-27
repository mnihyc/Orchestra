/** A conversation that is read again (a full resync, a reconnect, a changed history, a slow answer) stays on screen until
 * the fresh answer replaces it whole; only a refusal of the reader ends it. Its thread reads keep their last answer too. */
import { QueryObserver } from '@tanstack/react-query';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient, threadReadIdentity } from '@/api/query';
import { createRealtimeSocket } from '@/realtime/socket';
import { createThreadReadFeed } from '@/realtime/threadFeed';
import { followFeed } from '@/realtime/sync';
import { coldThreadKey, defaultThreadRead, newThreadReadScope, scopedThreadKey, threadReadRegistryKey, type ThreadReadRef } from '@/realtime/threadScope';
import { flushDeltas, setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer, threadReadState, threadViewOf,
  threadWriter } from '@/realtime/stores/thread';
import { Wire, answer, deferred, opening, projection, snapshot } from './continuation-fixtures';
import { SHARED, T, coordinator, ownedRows, serve } from './thread-read-fixtures';

const route = defaultThreadRead(T);
const stopped: (() => void)[] = [];

function setup() {
  const joined = coordinator(); stopped.push(() => joined.registry.clear());
  const wires: Wire[] = [];
  const socket = createRealtimeSocket({ url: () => 'ws://synthetic.test/api/v1/ws', token: () => 'synthetic-token', renew: () => Promise.resolve(true),
    onSuspended: () => undefined, random: () => 0.5, connect: () => { const wire = new Wire(); wires.push(wire); return wire; } });
  socket.start(); const wire = wires[0]; if (wire === undefined) throw new Error('Expected a native socket'); wire.ready(); stopped.push(() => socket.stop());
  const follow = (ref: ThreadReadRef) => {
    const follower = followFeed(socket, createThreadReadFeed(ref, ownedRows(ref)));
    stopped.push(joined.registry.use(threadReadRegistryKey(ref), () => follower, () => threadWriter.evict(ref)));
    return follower;
  };
  return { socket, wire, wires, follow };
}
function ack(wire: Wire, sub: { id: number; feed: string }) {
  const scope = threadViewOf(route).read?.projection; if (scope === undefined || scope === null) throw new Error('The opening was not admitted');
  wire.say({ op: 'subscribed', id: sub.id, feed: sub.feed, mode: 'delta', seq: threadViewOf(route).thread?.seq ?? 0, projection: scope });
}
/** The second opening of the thread waits for `gate` and answers with one fresh item in place of the first one. */
function slowSecondOpening(gate: Promise<Response | undefined>) {
  let openings = 0;
  return serve((request) => {
    if (!new URL(request.url).pathname.endsWith('/opening')) return undefined;
    openings += 1;
    if (openings === 1) return undefined;
    return gate.then((given) => {
      if (given instanceof Response) return given;
      const cut = snapshot(projection(), 30);
      return Response.json(opening({ ...cut, thread: { ...cut.thread, id: T }, items: [answer('fresh', 'The fresh answer', undefined, 30)] }));
    });
  });
}
const texts = () => [...threadViewOf(route).items.values()].map((item) => item.kind === 'agent_message' ? item.payload.text : '');

beforeEach(() => {
  vi.useFakeTimers(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});
afterEach(() => {
  for (const stop of stopped.splice(0).reverse()) stop(); flushDeltas(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
  vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
});

it('keeps the conversation through a full resync and a slow answer, then replaces it whole', async () => {
  const gate = deferred<undefined>(); slowSecondOpening(gate.promise);
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  expect(texts()).toEqual(['body-01950000-0000-7000-8000-000000000001']);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(5_000);
  const kept = threadViewOf(route);
  expect(kept.items.has(SHARED)).toBe(true); expect(kept.loaded).not.toBe('none');
  expect(threadReadState(kept)).toMatchObject({ readiness: 'loading', readOnly: true, kept: true });
  gate.resolve(undefined); await vi.advanceTimersByTimeAsync(0);
  const fresh = threadViewOf(route);
  expect(texts()).toEqual(['The fresh answer']);
  expect(fresh.retired).toBeUndefined();
  expect(threadReadState(fresh)).toMatchObject({ readiness: 'ready', readOnly: false, kept: false });
});

it('keeps the conversation when the server asks for a reload after a reconnect', async () => {
  const gate = deferred<undefined>(); slowSecondOpening(gate.promise);
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.onclose?.({ code: 1006 });
  await vi.advanceTimersByTimeAsync(1_000);
  const wire = rig.wires.at(-1); if (wire === undefined || wire === rig.wire) throw new Error('The socket must connect again'); wire.ready();
  const again = await wire.subscriptions.next();
  wire.say({ op: 'subscribed', id: again.id, feed: again.feed, mode: 'reload', seq: 20, projection: projection() });
  await vi.advanceTimersByTimeAsync(5_000);
  expect(threadViewOf(route).items.has(SHARED)).toBe(true);
  expect(threadReadState(threadViewOf(route))).toMatchObject({ readiness: 'loading', kept: true });
  gate.resolve(undefined); await vi.advanceTimersByTimeAsync(0);
  expect(texts()).toEqual(['The fresh answer']);
});

it('keeps the conversation while a changed history is read again, as when a turn starts', async () => {
  const gate = deferred<undefined>(); slowSecondOpening(gate.promise);
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  const next = { ...projection(), projection_revision: 2, execution_revision: 2 };
  rig.wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: 20, seq: 21, data: { projection: next } });
  await vi.advanceTimersByTimeAsync(5_000);
  expect(threadViewOf(route).items.has(SHARED)).toBe(true);
  expect(threadViewOf(route).retired).toBe(true);
  expect(threadReadState(threadViewOf(route)).kept).toBe(true);
  gate.resolve(undefined); await vi.advanceTimersByTimeAsync(0);
  expect(texts()).toEqual(['The fresh answer']);
});

it('ends what was kept when the reader is refused, and keeps it greyed when only the line failed', async () => {
  const refused = deferred<Response>(); slowSecondOpening(refused.promise);
  const rig = setup(); rig.follow(route); const sub = await rig.wire.subscriptions.next(); ack(rig.wire, sub);
  rig.wire.say({ op: 'resync', id: sub.id, feed: sub.feed, reload: true });
  await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(route).items.has(SHARED)).toBe(true);
  refused.resolve(Response.json({ type: 'orch:error:authz.denied', code: 'authz.denied', status: 403, title: 'Synthetic refusal', detail: 'No longer yours', request_id: 'synthetic' }, { status: 403 }));
  await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(route).items.size).toBe(0);
  expect(threadReadState(threadViewOf(route))).toMatchObject({ readiness: 'failed', kept: false });
});

it('carries a thread read answer over to the same read under a new epoch, never to another thread or a cleared cache', () => {
  const scope = newThreadReadScope(1, route), base = ['thread-outline', T];
  const before = scopedThreadKey(scope, base), after = scopedThreadKey({ ...scope, epoch: scope.epoch + 1, provisional: false }, base);
  expect(threadReadIdentity(before)).toBe(threadReadIdentity(after));
  expect(threadReadIdentity(coldThreadKey(1, base, route))).toBe(threadReadIdentity(after));
  expect(threadReadIdentity(scopedThreadKey({ ...scope, selection: 'older-version' }, base))).not.toBe(threadReadIdentity(after));
  expect(threadReadIdentity(['thread-outline', 'another-thread', 'continuation-read', 1, 0, null])).not.toBe(threadReadIdentity(after));
  expect(threadReadIdentity(['node', T])).toBeNull();
  queryClient.setQueryData(before, { entries: ['Turn 1'] });
  const observer = new QueryObserver(queryClient, { queryKey: before, queryFn: () => new Promise<{ entries: string[] }>(() => undefined), staleTime: Infinity });
  const off = observer.subscribe(() => undefined);
  try {
    observer.setOptions({ queryKey: after, queryFn: () => new Promise<{ entries: string[] }>(() => undefined), staleTime: Infinity, enabled: false });
    expect(observer.getCurrentResult()).toMatchObject({ data: { entries: ['Turn 1'] }, isPlaceholderData: true });
    queryClient.setQueryData(after, { entries: ['Turn 1', 'Turn 2'] });
    expect(observer.getCurrentResult()).toMatchObject({ data: { entries: ['Turn 1', 'Turn 2'] }, isPlaceholderData: false });
    const other = ['thread-outline', 'another-thread', 'continuation-read', scope.lifetime, 0, null, null, null, null, null];
    observer.setOptions({ queryKey: other, queryFn: () => new Promise<{ entries: string[] }>(() => undefined), staleTime: Infinity, enabled: false });
    expect(observer.getCurrentResult().data).toBeUndefined();
    observer.setOptions({ queryKey: after, queryFn: () => new Promise<{ entries: string[] }>(() => undefined), staleTime: Infinity, enabled: false });
    queryClient.clear();
    observer.setOptions({ queryKey: before, queryFn: () => new Promise<{ entries: string[] }>(() => undefined), staleTime: Infinity, enabled: false });
    expect(observer.getCurrentResult().data).toBeUndefined();
  } finally { off(); }
});
