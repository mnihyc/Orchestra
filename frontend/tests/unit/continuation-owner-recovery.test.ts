/** Real owned-only and socket entrypoints recover one current read, without manufacturing store resets. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { captureThreadExecution, captureThreadRead, isCurrentThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { fetchItem, fetchThreadSync, resumeQueue, sendInput } from '@/api/threads';
import { viewedItemQuery } from '@/api/threadHistory';
import type { ThreadInputCreateReply } from '@/generated/api-types';
import { createRealtimeSocket } from '@/realtime/socket';
import { applyThreadFeedRows, coordinateThreadReads, createThreadReadFeed } from '@/realtime/threadFeed';
import { EVICT_AFTER_MS, UNSUBSCRIBE_AFTER_MS, FeedRegistry, followFeed } from '@/realtime/sync';
import { flushDeltas, holdThreadRead, setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer,
  threadReadOnly, threadReadState, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { me, userRef } from '../doubles/fixtures';
import { threadInput } from '../doubles/thread-rows';
import { A, B, C, Wire, answer, deferred, opening, projection, snapshot } from './continuation-fixtures';

const stopped: (() => void)[] = [];
const id = snapshot().thread.id;
const tick = () => vi.advanceTimersByTimeAsync(0);

function transport(handler: (request: Request) => Promise<Response>) {
  const calls: Request[] = [];
  vi.stubGlobal('fetch',(input: RequestInfo | URL,init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input,init);
    calls.push(request); return handler(request);
  });
  return calls;
}
function rows(threadId: string) {
  return { rows: (data: Parameters<typeof applyThreadFeedRows>[1],reset: boolean) => { applyThreadFeedRows(threadId,data,reset); } };
}
function setup() {
  const registry = new FeedRegistry();
  const reload = coordinateThreadReads(registry,rows);
  stopped.push(() => registry.clear());
  return { registry,reload };
}
function response(request: Request,revision = 2,seq = 10) {
  const owner = new URL(request.url).searchParams.get('continuation_id');
  const scope = owner === null ? projection() : projection(owner,A,revision,false);
  return opening({ ...snapshot(scope,seq),items: [answer(`answer-${scope.viewed_continuation_id}`,scope.viewed_continuation_id,scope.viewed_continuation_id,seq)] });
}
function connect() {
  const wire = new Wire();
  const socket = createRealtimeSocket({ url: () => 'ws://synthetic.test/api/v1/ws',token: () => 'synthetic-token',
    renew: () => Promise.resolve(true),onSuspended: () => undefined,random: () => 0.5,connect: () => wire });
  socket.start(); wire.ready(); stopped.push(() => socket.stop());
  return { wire,socket };
}
function useStreaming(registry: FeedRegistry) {
  const { wire,socket } = connect();
  const follower = followFeed(socket,createThreadReadFeed(id,rows(id)));
  const release = registry.use(`thread:${id}`,() => follower,() => threadWriter.evict(id));
  stopped.push(release);
  return { wire,follower,release };
}
function ack(wire: Wire,sub: { id: number; feed: string },seq = 10) {
  wire.say({ op: 'subscribed',id: sub.id,feed: sub.feed,mode: 'delta',seq,projection: projection() });
}

beforeEach(() => {
  vi.useFakeTimers(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});
afterEach(() => {
  for (const stop of stopped.splice(0).reverse()) stop();
  flushDeltas(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
  vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
});

it('an actual held-only owner selects B and recovers a B409 through the installed coordinator without a follower or idle reads', async () => {
  const { registry } = setup(); const streaming = vi.spyOn(registry,'use');
  let stale = true; let branchRevision = 2;
  const calls = transport((request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith('/opening')) return Promise.resolve(Response.json(response(request,branchRevision)));
    if (path.endsWith('/old-a')) return Promise.resolve(Response.json(answer('old-a','Old admitted A',A)));
    if (stale) { stale = false; branchRevision = 3; return Promise.resolve(Response.json({ code: 'conflict.stale_revision',status: 409,title: 'Synthetic stale view',detail: 'Synthetic current projection changed',request_id: 'synthetic' },{ status: 409 })); }
    return Promise.resolve(Response.json(answer('item','B current',B)));
  });
  const release = holdThreadRead(id); stopped.push(release);
  await fetchThreadSync(id,null);
  const old = captureThreadRead(id);
  const oldRow = await fetchItem(id,'old-a');
  const selectedAt = calls.length;
  selectThreadContinuation(id,B); selectThreadContinuation(id,B);
  await tick();
  expect(calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(2);
  expect(threadReadState(threadViewOf(id))).toMatchObject({ selection: B,readiness: 'ready',readOnly: true });
  expect(threadViewOf(id).read?.projection).toMatchObject({ viewed_continuation_id: B,follows_active: false });
  expect(isCurrentThreadRead(old)).toBe(false);
  threadWriter.older(id,[oldRow],false);
  expect(threadViewOf(id).items.has(oldRow.id)).toBe(false);
  const query = viewedItemQuery(id,'item'); expect(query.enabled).toBe(true);
  if (typeof query.queryFn !== 'function') throw new Error('Native item query missing');
  await expect(query.queryFn({ client: queryClient,queryKey: query.queryKey,signal: new AbortController().signal,meta: undefined }))
    .rejects.toMatchObject({ code: 'conflict.stale_revision' });
  await tick();
  expect(threadReadState(threadViewOf(id)).readiness).toBe('ready');
  expect(calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(3);
  const current = viewedItemQuery(id,'item'); expect(current.enabled).toBe(true);
  if (typeof current.queryFn !== 'function') throw new Error('Current item query missing');
  const item = await current.queryFn({ client: queryClient,queryKey: current.queryKey,signal: new AbortController().signal,meta: undefined });
  expect(item.kind === 'agent_message' ? item.payload.text : '').toBe('B current');
  for (const request of calls.slice(selectedAt)) {
    const url = new URL(request.url); expect(request.method).toBe('GET'); expect(url.searchParams.get('continuation_id')).toBe(B);
  }
  expect(calls.filter((request) => new URL(request.url).pathname.endsWith('/items/item'))
    .map((request) => new URL(request.url).searchParams.get('projection_revision'))).toEqual(['2','3']);
  expect(threadViewOf(id).read?.projection?.projection_revision).toBe(3);
  expect(streaming).not.toHaveBeenCalled();
  const count = calls.length; await vi.advanceTimersByTimeAsync(60_000); expect(calls).toHaveLength(count);
});

it('a retained idle selection does not load until a visible HTTP owner actually returns', async () => {
  setup(); const calls = transport((request) => Promise.resolve(Response.json(response(request))));
  await fetchThreadSync(id,null);
  selectThreadContinuation(id,B); await tick(); await vi.advanceTimersByTimeAsync(60_000);
  expect(calls).toHaveLength(1); expect(threadViewOf(id).read?.ready).toBe(false);
  const release = holdThreadRead(id); stopped.push(release);
  await tick();
  expect(calls).toHaveLength(2); expect(threadViewOf(id).read?.projection?.viewed_continuation_id).toBe(B);
  expect(threadReadState(threadViewOf(id)).readiness).toBe('ready');
});

it('selection still recovers after the last real streaming owner leaves and its grace expires', async () => {
  const { registry } = setup(); const calls = transport((request) => Promise.resolve(Response.json(response(request))));
  const releaseHTTP = holdThreadRead(id); stopped.push(releaseHTTP);
  const { wire,release } = useStreaming(registry);
  const first = await wire.subscriptions.next(); ack(wire,first);
  release(); await vi.advanceTimersByTimeAsync(UNSUBSCRIBE_AFTER_MS);
  selectThreadContinuation(id,B); await tick();
  expect(calls).toHaveLength(2); expect(threadViewOf(id).read?.projection?.viewed_continuation_id).toBe(B);
  expect(wire.sent.filter((frame) => frame.op === 'sub')).toHaveLength(1);
  expect(threadReadState(threadViewOf(id)).readiness).toBe('ready');
});

it('replacing a held-only B opening neither waits for its ignored abort nor publishes its late body/error over C', async () => {
  setup(); const old = deferred<Response>();
  const calls = transport((request) => new URL(request.url).searchParams.get('continuation_id') === B ? old.promise : Promise.resolve(Response.json(response(request))));
  const release = holdThreadRead(id); stopped.push(release); await fetchThreadSync(id,null);
  selectThreadContinuation(id,B); await tick(); expect(calls).toHaveLength(2);
  selectThreadContinuation(id,C); await tick(); expect(calls).toHaveLength(3);
  const current = captureThreadRead(id);
  old.resolve(Response.json(opening(snapshot(projection(B,A,2,false))))); await tick();
  expect(isCurrentThreadRead(current)).toBe(true); expect(threadViewOf(id).read?.projection?.viewed_continuation_id).toBe(C);
  expect(threadViewOf(id).order).toEqual([`answer-${C}`]); expect(threadViewOf(id).error).toBeNull();
});

it('a streaming owner retires an in-flight held-only opening before making its own admitted window', async () => {
  const { registry } = setup(); const old = deferred<Response>(); let delayed = true;
  const calls = transport((request) => {
    if (new URL(request.url).searchParams.get('continuation_id') === B && delayed) { delayed = false; return old.promise; }
    return Promise.resolve(Response.json(response(request)));
  });
  const release = holdThreadRead(id); stopped.push(release); await fetchThreadSync(id,null);
  selectThreadContinuation(id,B); await tick(); expect(calls).toHaveLength(2);
  const { wire } = useStreaming(registry);
  const sub = await wire.subscriptions.next();
  wire.say({ op: 'subscribed',id: sub.id,feed: sub.feed,mode: 'delta',seq: 10,projection: projection(B,A,2,false) });
  old.resolve(Response.json(opening(snapshot(projection(B,A,2,false))))); await tick();
  expect(calls).toHaveLength(3); expect(threadViewOf(id).read?.projection?.viewed_continuation_id).toBe(B);
  expect(threadViewOf(id).order).toEqual([`answer-${B}`]); expect(threadViewOf(id).error).toBeNull();
});

it('a refused modern ack retires Send/Resume/positioned tickets and only explicit Retry restores active controls', async () => {
  const { registry,reload } = setup(); const pending = deferred<Response>(); let nativeSeq = 10;
  const created: ThreadInputCreateReply = { seq: 31,input: { ...threadInput('sent','Active words',{ author: userRef(me(1,'ada','Ada Lovelace')),position: '1' }),continuation_id: A,thread_id: id,seq_updated: 31 } };
  const calls = transport((request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith('/opening')) return Promise.resolve(Response.json(response(request,2,nativeSeq)));
    if (request.method === 'POST') return Promise.resolve(Response.json(created));
    return pending.promise;
  });
  const { wire,follower } = useStreaming(registry);
  const first = await wire.subscriptions.next();
  const before = captureThreadRead(id);
  const old = fetchItem(id,'late');
  const discarded = expect(old).rejects.toMatchObject({ name: 'AbortError' });
  wire.say({ op: 'subscribed',id: first.id,feed: first.feed,seq: 10,mode: 'delta' });
  expect(follower.lastSeq()).toBeNull(); expect(threadReadState(threadViewOf(id))).toMatchObject({ readiness: 'failed',readOnly: true });
  expect(threadReadOnly(threadViewOf(id))).toBe(true); expect(before.scope.controller.signal.aborted).toBe(true);
  expect(isCurrentThreadRead(before)).toBe(false); expect(() => captureThreadExecution(id)).toThrow('The thread read selection changed');
  await expect(sendInput(id,{ content: [{ type: 'text',text: 'Not sent' }],delivery: 'queue',clientId: 'refused-send' }))
    .rejects.toMatchObject({ name: 'AbortError' });
  await expect(resumeQueue(id,1)).rejects.toMatchObject({ name: 'AbortError' });
  await expect(fetchItem(id,'another')).rejects.toMatchObject({ name: 'AbortError' });
  expect(calls.filter((request) => request.method === 'POST')).toHaveLength(0);
  // Current global metadata may still update its title; that alone cannot admit this failed read.
  nativeSeq = 30;
  threadWriter.apply(id,{ thread: { ...snapshot().thread,seq: nativeSeq,name: 'Current metadata' } });
  expect(threadReadState(threadViewOf(id))).toMatchObject({ readiness: 'failed',readOnly: true });
  pending.resolve(Response.json(answer('late','Must not publish'))); await discarded;
  await vi.advanceTimersByTimeAsync(10_000);
  expect(calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(1);
  reload(id);
  const next = await wire.subscriptions.next(); ack(wire,next,nativeSeq);
  expect(threadReadState(threadViewOf(id))).toMatchObject({ readiness: 'ready',readOnly: false });
  expect(captureThreadExecution(id).scope.controller.signal.aborted).toBe(false);
  const result = await sendInput(id,{ content: [{ type: 'text',text: 'Active words' }],delivery: 'queue',clientId: 'after-retry' });
  expect(result).toEqual(created); expect(calls.filter((request) => request.method === 'POST')).toHaveLength(1);
  expect(calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))).toHaveLength(2);
});

it('a returning holder does not turn a refused protocol into an automatic HTTP-only retry', async () => {
  const { registry } = setup(); const calls = transport((request) => Promise.resolve(Response.json(response(request))));
  const release = holdThreadRead(id); const { wire,release: releaseStream } = useStreaming(registry);
  const sub = await wire.subscriptions.next();
  wire.say({ op: 'subscribed',id: sub.id,feed: sub.feed,seq: 10,mode: 'delta' });
  release(); releaseStream(); await vi.advanceTimersByTimeAsync(UNSUBSCRIBE_AFTER_MS);
  const releaseNext = holdThreadRead(id); stopped.push(releaseNext);
  await tick(); await vi.advanceTimersByTimeAsync(EVICT_AFTER_MS);
  expect(calls).toHaveLength(1); expect(threadReadState(threadViewOf(id))).toMatchObject({ readiness: 'failed',readOnly: true });
});

it('an ordinary capacity error retains the existing transport-error ticket policy', async () => {
  const { registry } = setup(); transport((request) => Promise.resolve(Response.json(response(request))));
  const { wire } = useStreaming(registry); const sub = await wire.subscriptions.next(); ack(wire,sub);
  const before = captureThreadRead(id);
  wire.say({ op: 'error',id: sub.id,feed: sub.feed,code: 'too_many_subscriptions',detail: 'Synthetic capacity refusal' });
  expect(threadViewOf(id).error).not.toBeNull(); expect(before.scope.controller.signal.aborted).toBe(false);
  expect(isCurrentThreadRead(before)).toBe(true); expect(threadReadOnly(threadViewOf(id))).toBe(false);
});
