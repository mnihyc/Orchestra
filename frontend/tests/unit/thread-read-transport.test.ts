/** One native socket carries independently owned reads, cursors and lifecycle fences. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { captureThreadExecution, captureThreadRead, isCurrentThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { fetchOutput, fetchThreadSync, sendInput } from '@/api/threads';
import { createRealtimeSocket } from '@/realtime/socket';
import { createThreadReadFeed, forgetThreadRead } from '@/realtime/threadFeed';
import { followFeed, UNSUBSCRIBE_AFTER_MS } from '@/realtime/sync';
import { defaultThreadRead, resultThreadRead, threadReadRegistryKey, type ThreadReadRef } from '@/realtime/threadScope';
import { flushDeltas, holdThreadRead, setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer,
  threadReadState, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { Wire, deferred, viewedPrompt } from './continuation-fixtures';
import { A, B, SHARED, T, bodyOf, coordinator, ownedRows, serve } from './thread-read-fixtures';

const route = defaultThreadRead(T), preview = resultThreadRead(T,'preview-tab'), second = resultThreadRead(T,'second-tab');
const stopped: (() => void)[] = [];
function setup() {
  const joined = coordinator(); stopped.push(() => joined.registry.clear());
  const wires: Wire[] = [];
  const socket = createRealtimeSocket({ url: () => 'ws://synthetic.test/api/v1/ws',token: () => 'synthetic-token',renew: () => Promise.resolve(true),
    onSuspended: () => undefined,random: () => 0.5,connect: () => { const wire = new Wire(); wires.push(wire); return wire; } });
  socket.start(); const wire = wires[0]; if (wire === undefined) throw new Error('Expected native socket'); wire.ready(); stopped.push(() => socket.stop());
  const follow = (ref: ThreadReadRef) => {
    const follower = followFeed(socket,createThreadReadFeed(ref,ownedRows(ref)));
    const release = joined.registry.use(threadReadRegistryKey(ref),() => follower,() => threadWriter.evict(ref));
    stopped.push(release); return { follower,release };
  };
  return { ...joined,socket,wire,wires,follow };
}
function ack(wire: Wire,sub: { id: number; feed: string },ref: ThreadReadRef) {
  const scope = threadViewOf(ref).read?.projection; if (scope === undefined || scope === null) throw new Error('Opening was not admitted');
  wire.say({ op: 'subscribed',id: sub.id,feed: sub.feed,mode: 'delta',seq: threadViewOf(ref).thread?.seq ?? 0,projection: scope });
}

beforeEach(() => {
  vi.useFakeTimers(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});
afterEach(() => {
  for (const stop of stopped.splice(0).reverse()) stop(); flushDeltas(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
  vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
});

it('route A and frozen B use distinct subscriptions on one socket with only the actual native feed', async () => {
  const server = serve(), rig = setup(); selectThreadContinuation(preview,B);
  rig.follow(route); rig.follow(preview);
  const first = await rig.wire.subscriptions.next(), next = await rig.wire.subscriptions.next();
  const previewSub = first.continuation_id === B ? first : next, routeSub = first.continuation_id === B ? next : first;
  expect(routeSub.feed).toBe(`thread:${T}`); expect(previewSub.feed).toBe(`thread:${T}`); expect(routeSub.id).not.toBe(previewSub.id);
  expect(routeSub).not.toHaveProperty('continuation_id'); expect(previewSub.continuation_id).toBe(B);
  ack(rig.wire,routeSub,route); ack(rig.wire,previewSub,preview);
  expect(rig.wires).toHaveLength(1); expect(server.calls).toHaveLength(2);
  rig.wire.say({ op: 'delta',id: routeSub.id,feed: routeSub.feed,item_id: SHARED,field: 'text',offset: `body-${A}`.length,len: 1,data: '!' });
  rig.wire.say({ op: 'delta',id: previewSub.id,feed: previewSub.feed,item_id: SHARED,field: 'text',offset: `body-${B}`.length,len: 1,data: '!' });
  flushDeltas();
  expect(bodyOf(threadViewOf(route).items.get(SHARED))).toBe(`body-${A}!`);
  expect(bodyOf(threadViewOf(preview).items.get(SHARED))).toBe(`body-${B}`);
});

it('follow-active resets independently while explicit B keeps its frozen window and only learns active metadata', async () => {
  const server = serve(), rig = setup(); selectThreadContinuation(preview,B);
  rig.follow(route); rig.follow(preview);
  const one = await rig.wire.subscriptions.next(), two = await rig.wire.subscriptions.next();
  const oldRoute = one.continuation_id === B ? two : one, stable = one.continuation_id === B ? one : two;
  ack(rig.wire,oldRoute,route); ack(rig.wire,stable,preview);
  const before = captureThreadRead(preview);
  server.state.active = B; server.state.revision = 2; server.state.seq = 30;
  rig.wire.say({ op: 'sync',id: oldRoute.id,feed: oldRoute.feed,prev: 20,seq: 30,
    data: { projection: { viewed_continuation_id: B,active_continuation_id: B,projection_revision: 2,execution_revision: 2,follows_active: true } } });
  rig.wire.say({ op: 'sync',id: stable.id,feed: stable.feed,prev: 20,seq: 30,
    data: { projection: { viewed_continuation_id: B,active_continuation_id: B,projection_revision: 2,execution_revision: 2,follows_active: false } } });
  const renewed = await rig.wire.subscriptions.next(); ack(rig.wire,renewed,route); await vi.advanceTimersByTimeAsync(0);
  expect(server.calls).toHaveLength(3); expect(renewed).not.toHaveProperty('continuation_id'); expect(renewed.since).toBe(30);
  expect(threadViewOf(route).read?.projection?.viewed_continuation_id).toBe(B);
  expect(isCurrentThreadRead(before)).toBe(true); expect(bodyOf(threadViewOf(preview).items.get(SHARED))).toBe(`body-${B}`);
});

it('true close joins registry retirement with eviction; same-key reopen ignores old frames and HTTP without harming route', async () => {
  const pending = deferred<Response>(); const server = serve((request) => new URL(request.url).pathname.endsWith('/output') ? pending.promise : undefined);
  const rig = setup(); selectThreadContinuation(preview,B); rig.follow(route); rig.follow(preview);
  const one = await rig.wire.subscriptions.next(), two = await rig.wire.subscriptions.next();
  const oldSub = one.continuation_id === B ? one : two, routeSub = one.continuation_id === B ? two : one;
  ack(rig.wire,routeSub,route); ack(rig.wire,oldSub,preview);
  const routeBefore = threadViewOf(route), old = captureThreadRead(preview);
  const output = fetchOutput(preview,SHARED,0,64), discarded = expect(output).rejects.toMatchObject({ name: 'AbortError' });
  forgetThreadRead(rig.registry,preview);
  expect(threadViewOf(preview).read).toBeNull(); expect(threadViewOf(route)).toBe(routeBefore);
  selectThreadContinuation(preview,B); rig.follow(preview);
  const fresh = await rig.wire.subscriptions.next(); ack(rig.wire,fresh,preview);
  rig.wire.say({ op: 'subscribed',id: oldSub.id,feed: oldSub.feed,mode: 'reload',seq: 99 });
  rig.wire.say({ op: 'closed',id: oldSub.id,feed: oldSub.feed,reason: 'revoked' });
  rig.wire.say({ op: 'delta',id: oldSub.id,feed: oldSub.feed,item_id: SHARED,field: 'text',offset: 0,len: 3,data: 'old' });
  pending.resolve(new Response('Old closed output')); await discarded; flushDeltas();
  expect(captureThreadRead(preview).scope.lifetime).not.toBe(old.scope.lifetime); expect(isCurrentThreadRead(old)).toBe(false);
  expect(threadViewOf(route)).toBe(routeBefore); expect(threadViewOf(preview).closed).toBeNull();
  expect(bodyOf(threadViewOf(preview).items.get(SHARED))).toBe(`body-${B}`);
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(3);
  expect(rig.wires).toHaveLength(1);
});

it('protocol rejection and explicit Retry affect only the refused active-follow owner', async () => {
  const server = serve(), rig = setup(); rig.follow(route);
  const routeSub = await rig.wire.subscriptions.next(); ack(rig.wire,routeSub,route);
  rig.follow(second); const refused = await rig.wire.subscriptions.next(); const before = captureThreadRead(route), old = captureThreadRead(second);
  rig.wire.say({ op: 'subscribed',id: refused.id,feed: refused.feed,mode: 'delta',seq: 20 });
  expect(threadReadState(threadViewOf(second))).toMatchObject({ readiness: 'failed',readOnly: true });
  expect(isCurrentThreadRead(old)).toBe(false); expect(isCurrentThreadRead(before)).toBe(true);
  await expect(sendInput(second,{ content: [{ type: 'text',text: 'No work' }],delivery: 'queue',clientId: 'not-sent' })).rejects.toMatchObject({ name: 'AbortError' });
  expect(captureThreadExecution(route).ref.threadId).toBe(T);
  await vi.advanceTimersByTimeAsync(10_000); expect(server.calls).toHaveLength(2);
  rig.reload(second); const next = await rig.wire.subscriptions.next(); ack(rig.wire,next,second);
  expect(server.calls).toHaveLength(3); expect(isCurrentThreadRead(before)).toBe(true);
  expect(threadReadState(threadViewOf(second))).toMatchObject({ readiness: 'ready',readOnly: false });
  expect(server.calls.every((request) => request.method === 'GET')).toBe(true);
});

it('reconnect retains each independent selector/cursor on the same replacement connection', async () => {
  serve(); const rig = setup(); selectThreadContinuation(preview,B); rig.follow(route); rig.follow(preview);
  const one = await rig.wire.subscriptions.next(), two = await rig.wire.subscriptions.next();
  ack(rig.wire,one,one.continuation_id === B ? preview : route); ack(rig.wire,two,two.continuation_id === B ? preview : route);
  rig.wire.onclose?.({ code: 1006 }); await vi.advanceTimersByTimeAsync(500);
  const next = rig.wires[1]; if (next === undefined) throw new Error('Expected reconnect'); next.ready();
  const again = await next.subscriptions.next(), other = await next.subscriptions.next();
  expect([again.feed,other.feed]).toEqual([`thread:${T}`,`thread:${T}`]);
  expect([again.continuation_id,other.continuation_id]).toContain(B);
  expect([again.continuation_id,other.continuation_id]).toContain(undefined);
  expect([again.since,other.since]).toEqual([20,20]); expect(again.id).not.toBe(one.id); expect(other.id).not.toBe(two.id);
});

it('hiding one streaming owner cannot stop another same-thread owner or turn retained idle selection into a read', async () => {
  const server = serve(), rig = setup(); selectThreadContinuation(preview,B);
  const routeOwner = rig.follow(route), hidden = rig.follow(preview);
  const one = await rig.wire.subscriptions.next(), two = await rig.wire.subscriptions.next();
  ack(rig.wire,one,one.continuation_id === B ? preview : route); ack(rig.wire,two,two.continuation_id === B ? preview : route);
  const before = captureThreadRead(route); hidden.release(); await vi.advanceTimersByTimeAsync(UNSUBSCRIBE_AFTER_MS);
  const closedId = one.continuation_id === B ? one.id : two.id;
  expect(rig.wire.sent.some((frame) => frame.op === 'unsub' && frame.id === closedId)).toBe(true);
  expect(isCurrentThreadRead(before)).toBe(true); expect(routeOwner.follower.lastSeq()).toBe(20);
  // No follower/visible HTTP owner remains for this preview, so selection is retained only.
  selectThreadContinuation(preview,null); await vi.advanceTimersByTimeAsync(0); expect(server.calls).toHaveLength(2);
  const release = holdThreadRead(preview); stopped.push(release); await vi.advanceTimersByTimeAsync(0);
  expect(server.calls).toHaveLength(3); expect(isCurrentThreadRead(before)).toBe(true);
});


it('authority rebase preserves only each requested source, reloads owned views and never falls an idle B back to A', async () => {
  const server = serve(), rig = setup(); selectThreadContinuation(preview,B);
  rig.follow(route); rig.follow(preview);
  const one = await rig.wire.subscriptions.next(), two = await rig.wire.subscriptions.next();
  ack(rig.wire,one,one.continuation_id === B ? preview : route); ack(rig.wire,two,two.continuation_id === B ? preview : route);
  selectThreadContinuation(second,B); await fetchThreadSync(second,null);
  const oldRoute = captureThreadRead(route), oldPreview = captureThreadRead(preview);
  rig.registry.rebase();
  const again = await rig.wire.subscriptions.next(), other = await rig.wire.subscriptions.next();
  ack(rig.wire,again,again.continuation_id === B ? preview : route); ack(rig.wire,other,other.continuation_id === B ? preview : route);
  expect(isCurrentThreadRead(oldRoute)).toBe(false); expect(isCurrentThreadRead(oldPreview)).toBe(false);
  expect(threadViewOf(preview).read?.selection).toBe(B); expect(bodyOf(threadViewOf(preview).items.get(SHARED))).toBe(`body-${B}`);
  expect(threadViewOf(second).read?.selection).toBe(B); expect(threadViewOf(second).thread).toBeNull();
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(5);
  expect([again.continuation_id,other.continuation_id]).toContain(B); expect([again.continuation_id,other.continuation_id]).toContain(undefined);
  const release = holdThreadRead(second); stopped.push(release); await vi.advanceTimersByTimeAsync(0);
  expect(server.calls).toHaveLength(6); expect(threadViewOf(second).read?.selection).toBe(B);
  expect(bodyOf(threadViewOf(second).items.get(SHARED))).toBe(`body-${B}`);
});


it('retains the admitted prompt-status frame sequence without a thread row and only for its read owner', async () => {
  serve(); const rig = setup(); selectThreadContinuation(preview, B);
  rig.follow(route); rig.follow(preview);
  const first = await rig.wire.subscriptions.next(), next = await rig.wire.subscriptions.next();
  const sub = first.continuation_id === B ? first : next, rootSub = first.continuation_id === B ? next : first;
  ack(rig.wire, sub, preview); ack(rig.wire, rootSub, route);
  const routeBefore = threadViewOf(route), threadBefore = threadViewOf(preview).thread;
  const scope = threadViewOf(preview).read?.projection;
  const failed = viewedPrompt('failed');
  rig.wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: 20, seq: 21,
    data: { projection: scope, viewed_prompt: failed } });
  await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(preview).viewedPromptSeq).toBe(21);
  expect(threadViewOf(preview).thread).toBe(threadBefore); expect(threadViewOf(route)).toBe(routeBefore);
  const newer = { ...failed, job: { ...failed.job!, id: 'newer-terminal-job' } };
  rig.wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: 21, seq: 22,
    data: { projection: scope, viewed_prompt: newer } });
  rig.wire.say({ op: 'sync', id: sub.id, feed: sub.feed, prev: 22, seq: 23, data: { projection: scope } });
  await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(preview).viewedPromptSeq).toBe(22);
  expect(threadViewOf(preview).viewedPrompt?.job?.id).toBe('newer-terminal-job');
  threadWriter.apply(preview, { seq: 21, viewed_prompt: failed }, captureThreadRead(preview));
  expect(threadViewOf(preview).viewedPrompt?.job?.id).toBe('newer-terminal-job');
});
