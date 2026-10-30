/** Independent owners exercise the real opening/window/query/receipt paths over native-shaped cuts. */
import { readAdmittedRaw } from './raw-observer-fixture';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { FeedRegistry, type FeedFollower } from '@/realtime/sync';
import { queryClient } from '@/api/query';
import { settingsKey } from '@/api/settings';
import { captureThreadRead, isCurrentThreadRead, selectThreadContinuation, threadReadQueryKey, stampThreadRows } from '@/api/threadRead';
import { fetchItem, fetchItems, fetchOutput, fetchSubagentItems, fetchThreadSync, markRead, sendInput, threadKeys } from '@/api/threads';
import { exportViewedThreadFile, viewedItemQuery, viewedRawFramesQuery } from '@/api/threadHistory';
import type { Draft, ThreadLimits, ThreadParameters } from '@/generated/api-types';
import { holdThreadRead, flushDeltas, setGapHandler, setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer,
  threadReadState, threadViewOf, threadWriter, useThreadStore } from '@/realtime/stores/thread';
import { defaultThreadRead, resultThreadRead, sameThreadRead, threadReadCacheKey, type ThreadReadCapture } from '@/realtime/threadScope';
import { AT } from '../doubles/thread-rows';
import { deferred, snapshot } from './continuation-fixtures';
import { applyThreadFeedRows } from '@/realtime/threadFeed';
import { A, B, C, OTHER, SHARED, OLDER, T, bodyOf, coordinator, serve } from './thread-read-fixtures';

const route = defaultThreadRead(T), left = resultThreadRead(T,'tab-left'), right = resultThreadRead(T,'tab-right');
const stopped: (() => void)[] = [];
function setup() { const joined = coordinator(); stopped.push(() => joined.registry.clear()); return joined; }
async function open(target: typeof route,selection: string | null) {
  selectThreadContinuation(target,selection); return fetchThreadSync(target,null);
}

beforeEach(() => {
  vi.useFakeTimers(); threadWriter.clear(); queryClient.clear();
  setGapHandler(() => undefined); setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});
afterEach(() => {
  for (const stop of stopped.splice(0).reverse()) stop();
  flushDeltas(); threadWriter.clear(); queryClient.clear(); vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
  setGapHandler(() => undefined); setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); setThreadReadHolder(() => () => undefined);
});

it('keeps the actual route and a frozen same-thread preview independent while their windows move', async () => {
  setup(); const server = serve();
  await open(route,null); await open(left,B);
  expect(bodyOf(threadViewOf(route).items.get(SHARED))).toBe(`body-${A}`);
  expect(bodyOf(threadViewOf(left).items.get(SHARED))).toBe(`body-${B}`);
  const routeBefore = threadViewOf(route);
  const page = await fetchItems(left,{ around: OLDER });
  threadWriter.window(left,page.items,page.has_older,page.has_newer);
  expect(threadViewOf(left).order).toEqual([OLDER]);
  expect(threadViewOf(route)).toBe(routeBefore);
  const older = await fetchItems(left,{ before: 8 }); threadWriter.older(left,older.items,older.has_older);
  await fetchThreadSync(left,null);
  expect(threadViewOf(route)).toBe(routeBefore); expect(threadViewOf(route).read?.selection).toBeNull();
  expect(server.calls.every((request) => new URL(request.url).pathname.startsWith(`/api/v1/threads/${T}/`))).toBe(true);
  expect(server.calls.every((request) => request.method === 'GET')).toBe(true);
});

it('equal branch owners retain separate anchored windows, including the route default wrapper', async () => {
  setup(); serve(); await open(route,null); await open(left,B); await open(right,B);
  const rightBefore = threadViewOf(right);
  const page = await fetchItems(left,{ around: OLDER }); threadWriter.window(left,page.items,false,true);
  expect(threadViewOf(right)).toBe(rightBefore); expect(threadViewOf(right).order).toEqual([SHARED]);
  expect(threadViewOf(T)).toBe(threadViewOf(route));
  expect(threadReadQueryKey(left,['thread-items',T])).not.toEqual(threadReadQueryKey(right,['thread-items',T]));
});

it('a preview opening follows active or explicit history without hydrating any shared control cache', async () => {
  setup(); serve(); await open(route,null);
  const draft: Draft = { content: [{ type: 'text',text: 'Unsaved route draft' }],delivery: 'queue',native_action: null,review_comments: [],updated_at: AT };
  const limits: ThreadLimits = { bypass_all: false,reached: [] };
  const parameters: ThreadParameters = { approval_mode: 'ask',approval_reviewer: null,parameters: [],
    effort: { source: { scope: 'default',node: null },supported: true,value: null,writable: false },
    model: { source: { scope: 'default',node: null },options: [],value: null,writable: false } };
  queryClient.setQueryData(threadKeys.draft(T),draft); queryClient.setQueryData(threadKeys.parameters(T),parameters);
  queryClient.setQueryData([...threadKeys.limits(T),null],limits);
  const settings = queryClient.getQueryData(settingsKey(T));
  const nativeThread = queryClient.getQueryData(['node',T]); expect(nativeThread).toBeDefined();
  await open(left,B); await open(right,null);
  expect(queryClient.getQueryData(threadKeys.draft(T))).toBe(draft);
  expect(queryClient.getQueryData(threadKeys.parameters(T))).toBe(parameters);
  expect(queryClient.getQueryData([...threadKeys.limits(T),null])).toBe(limits);
  expect(queryClient.getQueryData(settingsKey(T))).toBe(settings);
  expect(queryClient.getQueryData(['node',T])).toBe(nativeThread);
  expect(queryClient.getQueryData(threadKeys.outline(left))).toBeDefined();
  expect(queryClient.getQueryData(threadKeys.outline(left))).not.toBe(queryClient.getQueryData(threadKeys.outline(route)));
});

it('rejects a route capture used for a same-thread preview before any request or row mutation', async () => {
  setup(); const server = serve(); await open(route,null); await open(left,B);
  const captured = captureThreadRead(route), before = server.calls.length;
  await expect(fetchItem(left,SHARED,undefined,captured)).rejects.toMatchObject({ name: 'AbortError' });
  await expect(fetchOutput(left,SHARED,0,32,undefined,captured)).rejects.toMatchObject({ name: 'AbortError' });
  await expect(exportViewedThreadFile(left,{ format: 'markdown',detail: 'normal',include: [] },undefined,captured)).rejects.toMatchObject({ name: 'AbortError' });
  const row = await fetchItem(route,SHARED); threadWriter.older(left,[row],false);
  expect(bodyOf(threadViewOf(left).items.get(SHARED))).toBe(`body-${B}`);
  expect(server.calls).toHaveLength(before+1);
});

it('keeps cold owner keys distinct and constructors/query factories pure without a current selector map', () => {
  const before = useThreadStore.getState();
  const one = resultThreadRead(T,'tab-a'),two = resultThreadRead(T,'tab-b');
  expect(threadReadQueryKey(one,['thread-item',T,SHARED])).not.toEqual(threadReadQueryKey(two,['thread-item',T,SHARED]));
  viewedItemQuery(one,SHARED); viewedRawFramesQuery(two,'segment');
  expect(useThreadStore.getState()).toBe(before);
  expect(threadReadCacheKey(resultThreadRead(T,T))).not.toBe(threadReadCacheKey(route));
  expect(threadReadCacheKey(resultThreadRead(OTHER,'tab-a'))).not.toBe(threadReadCacheKey(one));
  expect(sameThreadRead(defaultThreadRead(T),T)).toBe(true); expect(sameThreadRead(one,two)).toBe(false);
  expect(Object.isFrozen(one) && Object.isFrozen(one.key)).toBe(true);
});

it('fences a late output and old query across preview B->C->B while route A remains current', async () => {
  setup(); const pending = deferred<Response>();
  const server = serve((request) => new URL(request.url).pathname.endsWith('/output') ? pending.promise : undefined);
  await open(route,null); await open(left,B);
  const routeBefore = threadViewOf(route), old = captureThreadRead(left), options = viewedItemQuery(left,SHARED);
  const output = fetchOutput(left,SHARED,0,32); const discarded = expect(output).rejects.toMatchObject({ name: 'AbortError' });
  selectThreadContinuation(left,C); selectThreadContinuation(left,B); await fetchThreadSync(left,null);
  pending.resolve(new Response('Old B output')); await discarded;
  expect(isCurrentThreadRead(old)).toBe(false); expect(threadViewOf(route)).toBe(routeBefore);
  const before = server.calls.length;
  if (typeof options.queryFn !== 'function') throw new Error('Expected real item query');
  await expect(options.queryFn({ client: queryClient,queryKey: options.queryKey,signal: new AbortController().signal,meta: undefined })).rejects.toMatchObject({ name: 'AbortError' });
  expect(server.calls).toHaveLength(before); expect(bodyOf(threadViewOf(left).items.get(SHARED))).toBe(`body-${B}`);
});

it('scopes output, export, raw pages and child-stream reads to the owner using only native URL identities', async () => {
  setup(); const server = serve(); await open(route,null); await open(left,B); const before = server.calls.length;
  expect(await fetchOutput(left,SHARED,0,64)).toBe(`output-${B}`);
  const saved = await exportViewedThreadFile(left,{ format: 'markdown',detail: 'normal',include: [] }); expect(await saved.blob.text()).toBe(`export-${B}`);
  const child = await fetchSubagentItems(left,'child-stream',100); expect(child.items[0]?.stream).toBe('child-stream');
  await readAdmittedRaw(left, 'segment');
  for (const request of server.calls.slice(before)) {
    const url = new URL(request.url); expect(url.pathname.startsWith(`/api/v1/threads/${T}/`)).toBe(true);
    expect(url.searchParams.get('continuation_id')).toBe(B); expect(url.searchParams.get('projection_revision')).toBe('2');
  }
  expect(threadViewOf(route).order).toEqual([SHARED]);
});

it('personal read markers update matching native continuations across owners, never another branch', async () => {
  setup(); serve(); await open(route,null); await open(left,B); await open(right,B);
  await markRead(left,12);
  expect(threadViewOf(left).thread?.my_last_read_seq).toBe(12); expect(threadViewOf(right).thread?.my_last_read_seq).toBe(12);
  expect(threadViewOf(route).thread?.my_last_read_seq).toBe(0);
  threadWriter.readStateAll(T,A,9); expect(threadViewOf(route).thread?.my_last_read_seq).toBe(9);
  expect(threadViewOf(left).openedReadSeq).toBe(0);
});

it('explicit preview action callbacks cannot borrow active route eligibility', async () => {
  setup(); const server = serve(); await open(route,null); await open(left,B); const before = server.calls.length;
  await expect(sendInput(left,{ content: [{ type: 'text',text: 'No native work' }],delivery: 'queue',clientId: 'not-sent-preview' })).rejects.toMatchObject({ name: 'AbortError' });
  expect(server.calls).toHaveLength(before); expect(isCurrentThreadRead(captureThreadRead(route))).toBe(true);
});

it('changing the reader retires each owner independently and old receipts cannot enter either fresh view', async () => {
  setup(); serve(); await open(route,null); await open(left,B);
  const oldRoute = captureThreadRead(route), oldPreview = captureThreadRead(left), oldRow = await fetchItem(left,SHARED);
  const reader = client.session.getState().readerVersion;
  try {
    client.session.setState({ readerVersion: (reader ?? 0)+1 });
    await open(route,null); await open(left,B);
    expect(isCurrentThreadRead(oldRoute)).toBe(false); expect(isCurrentThreadRead(oldPreview)).toBe(false);
    threadWriter.older(left,[oldRow],false); expect(bodyOf(threadViewOf(left).items.get(SHARED))).toBe(`body-${B}`);
    expect(captureThreadRead(left).scope.lifetime).not.toBe(oldPreview.scope.lifetime);
  } finally { client.session.setState({ readerVersion: reader }); }
});


it('held-only selection and409 recovery is owned by the preview, without a route reload', async () => {
  setup(); const server = serve(); await open(route,null);
  const routeBefore = threadViewOf(route), release = holdThreadRead(left); stopped.push(release);
  selectThreadContinuation(left,B); await vi.advanceTimersByTimeAsync(0);
  expect(threadReadState(threadViewOf(left)).readiness).toBe('ready');
  server.state.selectedRevision = 3;
  const stale = viewedItemQuery(left,SHARED); if (typeof stale.queryFn !== 'function') throw new Error('Expected item query');
  await expect(stale.queryFn({ client: queryClient,queryKey: stale.queryKey,signal: new AbortController().signal,meta: undefined }))
    .rejects.toMatchObject({ code: 'conflict.stale_revision' });
  await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(left).read?.projection?.projection_revision).toBe(3);
  expect(threadViewOf(route)).toBe(routeBefore);
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))
    .map((request) => new URL(request.url).searchParams.get('continuation_id'))).toEqual([null,B,B]);
  const count = server.calls.length; await vi.advanceTimersByTimeAsync(60_000); expect(server.calls).toHaveLength(count);
});


it('modern HTTP identity refusal retires just that owner while the active route remains ready', async () => {
  setup(); const server = serve((request) => new URL(request.url).pathname.endsWith('/items')
    ? Promise.resolve(Response.json({ items: [],turns: [],segments: [],has_older: false,has_newer: false })) : undefined);
  await open(route,null); await open(right,null);
  const routeTicket = captureThreadRead(route), old = captureThreadRead(right);
  await expect(fetchItems(right,{ around: SHARED })).rejects.toThrow('The reply did not identify the selected history');
  expect(threadReadState(threadViewOf(right))).toMatchObject({ readiness: 'failed',readOnly: true });
  expect(isCurrentThreadRead(old)).toBe(false); expect(isCurrentThreadRead(routeTicket)).toBe(true);
  const before = server.calls.length;
  await expect(sendInput(right,{ content: [{ type: 'text',text: 'Do not send' }],delivery: 'queue',clientId: 'not-sent-http-refusal' }))
    .rejects.toMatchObject({ name: 'AbortError' });
  expect(server.calls).toHaveLength(before);
});


it('native thread identity and local owner identity are checked separately before row publication', async () => {
  setup(); serve(); await open(route,null); await open(left,B);
  const before = threadViewOf(left), routeRows = stampThreadRows(snapshot(),captureThreadRead(route));
  expect(applyThreadFeedRows(left,routeRows,false)).toBeNull(); expect(threadViewOf(left)).toBe(before);
  const wrong = { ...snapshot().thread,id: OTHER,seq: 99 };
  threadWriter.apply(left,{ thread: wrong },captureThreadRead(left));
  expect(threadViewOf(left)).toBe(before); expect(threadViewOf(left).thread?.id).toBe(T);
});


it('an explicit preview delta uses its captured gap ticket without sending its local key as a native id', async () => {
  setup(); serve(); await open(route,null); await open(right,null);
  let repaired: ThreadReadCapture | undefined;
  const nativeIds: string[] = [];
  setGapHandler((native,_item,captured) => { nativeIds.push(native); repaired = captured; });
  threadWriter.delta(right,{ item_id: SHARED,field: 'text',offset: 999,len: 1,data: '!' },captureThreadRead(right)); flushDeltas();
  expect(nativeIds).toEqual([T]); expect(repaired).toBeDefined();
  if (repaired === undefined) throw new Error('Expected owner ticket');
  expect(sameThreadRead(repaired.ref,right)).toBe(true); expect(isCurrentThreadRead(repaired)).toBe(true);
  expect(threadViewOf(route).gaps.size).toBe(0); expect(threadViewOf(right).gaps.has(SHARED)).toBe(true);
});

it('a retained owner locator cannot mint a current receipt for an unsourced delta after away and back', async () => {
  setup(); serve(); await open(right,null);
  const old = captureThreadRead(right);
  await open(right,B); await open(right,null);
  const piece = { item_id: SHARED,field: 'text' as const,offset: `body-${A}`.length,len: 1,data: '!' };
  threadWriter.delta(right,piece,old); threadWriter.delta(right,piece); flushDeltas();
  expect(bodyOf(threadViewOf(right).items.get(SHARED))).toBe(`body-${A}`);
  threadWriter.delta(right,piece,captureThreadRead(right)); flushDeltas();
  expect(bodyOf(threadViewOf(right).items.get(SHARED))).toBe(`body-${A}!`);
});


it('a credential rebase removes every old body and ticket before reopening its explicit source', async () => {
  setup(); const server = serve(); await open(route,null); await open(left,B);
  const reader = client.session.getState().readerVersion, old = captureThreadRead(left);
  try {
    const next = (reader ?? 0)+1; client.session.setState({ readerVersion: next }); threadWriter.rebaseAll(next);
    expect(threadViewOf(route).thread).toBeNull(); expect(threadViewOf(left).thread).toBeNull();
    expect(threadViewOf(route).items.size).toBe(0); expect(threadViewOf(left).items.size).toBe(0);
    expect(threadViewOf(left).read?.selection).toBe(B); expect(threadViewOf(left).read?.projection).toBeNull();
    expect(isCurrentThreadRead(old)).toBe(false);
    await fetchThreadSync(left,null);
    expect(bodyOf(threadViewOf(left).items.get(SHARED))).toBe(`body-${B}`);
    expect(new URL(server.calls.at(-1)?.url ?? 'http://invalid').searchParams.get('continuation_id')).toBe(B);
  } finally { client.session.setState({ readerVersion: reader }); }
});

it('generic registry callers retain their original evict-and-reload authority behavior', () => {
  const registry = new FeedRegistry(), evict = vi.fn(), reload = vi.fn(), stop = vi.fn();
  const follower: FeedFollower = { lastSeq: () => 1,reload,stop,refresh: () => undefined };
  const release = registry.use('org:synthetic',() => follower,evict);
  registry.rebase(); expect(evict).toHaveBeenCalledTimes(1); expect(reload).toHaveBeenCalledTimes(1); expect(stop).not.toHaveBeenCalled();
  release(); registry.clear();
});


it('authority rebase also reopens an actually held HTTP-only B once without loading an idle route', async () => {
  const { registry } = setup(); const server = serve(); await open(route,null);
  const release = holdThreadRead(left); stopped.push(release); selectThreadContinuation(left,B); await vi.advanceTimersByTimeAsync(0);
  expect(threadViewOf(left).read?.selection).toBe(B); const old = captureThreadRead(left);
  registry.rebase(); await vi.advanceTimersByTimeAsync(0);
  expect(isCurrentThreadRead(old)).toBe(false); expect(threadReadState(threadViewOf(left)).readiness).toBe('ready');
  expect(threadViewOf(left).read?.selection).toBe(B); expect(threadViewOf(route).thread).toBeNull();
  expect(server.calls.filter((request) => new URL(request.url).pathname.endsWith('/opening'))
    .map((request) => new URL(request.url).searchParams.get('continuation_id'))).toEqual([null,B,B]);
});
