/** Real API helpers over bounded synthetic transport; selection never calls execution or versions discovery. */
import { readAdmittedRaw } from './raw-observer-fixture';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { client } from '@/api/client';
import { queryClient } from '@/api/query';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { catchupQuery, fetchItem, fetchItems, fetchOutput, fetchThreadSync, loadNextThreadInputs,
  markRead, outlineQuery, pinItem, segmentsQuery, sendInput, threadKeys, turnsQuery } from '@/api/threads';
import { exportViewedThreadFile, viewedItemQuery, viewedRawFramesQuery } from '@/api/threadHistory';
import type { Catchup, ThreadInputCreateReply, ThreadInputPage, ThreadItemListReply, ThreadRawListReply } from '@/generated/api-types';
import { setThreadReadChangeHandler, setThreadReadRetainer, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { threadInput } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { turn, userMessage } from '../doubles/thread-rows';
import { A, B, Signals, answer, deferred, opening, projection, snapshot, viewedPrompt } from './continuation-fixtures';

const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));
const inputId = (index: number) => `01950000-0000-7000-8001-${String(index).padStart(12,'0')}`;
const queued = (index: number) => ({ ...threadInput(inputId(index), `Pending ${index}`, { author: ADA, position: String(index + 1) }), continuation_id: B });

function http(handler: (request: Request) => Promise<Response>) {
  const calls: Request[] = [];
  const requests = new Signals<Request>();
  const fetch = vi.fn((input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const request = input instanceof Request ? input : new Request(input, init);
    calls.push(request); requests.emit(request); return handler(request);
  });
  vi.stubGlobal('fetch', fetch);
  return { calls, requests };
}

function installed(scope = projection()) {
  const sync = snapshot(scope); const id = sync.thread.id;
  if (!scope.follows_active) selectThreadContinuation(id, scope.viewed_continuation_id);
  threadWriter.reset(id, sync, false, captureThreadRead(id));
  return { id, sync };
}

beforeEach(() => {
  vi.useFakeTimers(); threadWriter.clear(); queryClient.clear();
  setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined);
});
afterEach(() => {
  threadWriter.clear(); queryClient.clear(); vi.unstubAllGlobals(); vi.clearAllTimers(); vi.useRealTimers();
});

it('uses one bounded opening for inactive history and durable viewed status, with no queue or versions discovery reads', async () => {
  const scope = projection(B, A, 4, false); const sync = snapshot(scope);
  const pending = queued(1);
  const response = opening({ ...sync, inputs: [pending], ...{ viewed_prompt: viewedPrompt('failed') } });
  const transport = http(() => Promise.resolve(Response.json(response)));
  queryClient.setQueryData(threadKeys.draft(sync.thread.id), { content: [{ type: 'text', text: 'Unsaved draft' }], delivery: 'queue' });
  selectThreadContinuation(sync.thread.id, B);
  const read = await fetchThreadSync(sync.thread.id, null);
  expect(read.items).toEqual([]);
  expect(threadViewOf(sync.thread.id).viewedPrompt?.status).toBe('failed');
  expect(threadViewOf(sync.thread.id).inputs.get(pending.id)?.state).toBe('pending');
  expect(queryClient.getQueryData(threadKeys.draft(sync.thread.id))).toMatchObject({ content: [{ text: 'Unsaved draft' }] });
  expect(transport.calls).toHaveLength(1);
  const url = new URL(transport.calls[0]?.url ?? 'http://invalid');
  expect(url.pathname).toBe(`/api/v1/threads/${sync.thread.id}/opening`);
  expect(url.searchParams.get('continuation_id')).toBe(B);
  expect(url.searchParams.get('limit')).toBe('200');
  expect(url.searchParams.has('projection_revision')).toBe(false);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(transport.calls).toHaveLength(1);
});

it('retires an old opening before it can hydrate another selection or overwrite its draft/main metadata', async () => {
  const sync = snapshot(); const id = sync.thread.id; const old = deferred<Response>();
  const current = opening({ ...snapshot(projection(B, A, 2, false)), items: [answer('current', 'Branch B', B)] });
  const transport = http((request) => new URL(request.url).searchParams.get('continuation_id') === B
    ? Promise.resolve(Response.json(current)) : old.promise);
  const pending = fetchThreadSync(id, null);
  await transport.requests.next();
  const refusal = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  selectThreadContinuation(id, B);
  queryClient.setQueryData(threadKeys.draft(id), { content: [{ type: 'text', text: 'Keep this draft' }], delivery: 'queue' });
  await fetchThreadSync(id, null);
  old.resolve(Response.json(opening(sync))); await refusal;
  expect(threadViewOf(id).order).toEqual(['current']);
  expect(threadViewOf(id).read?.selection).toBe(B);
  expect(queryClient.getQueryData(threadKeys.draft(id))).toMatchObject({ content: [{ text: 'Keep this draft' }] });
  expect(transport.calls).toHaveLength(2);
});

it('fences a page that ignored cancellation across repeated away/back instead of relying on the same native id', async () => {
  const { id } = installed(); const old = deferred<Response>();
  const transport = http(() => old.promise);
  const pending = fetchItems(id, { before: 5 });
  await transport.requests.next();
  const refusal = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  selectThreadContinuation(id, B); selectThreadContinuation(id, A);
  const current = snapshot(projection(A, B, 1, false));
  threadWriter.reset(id, { ...current, items: [answer('fresh', 'Current A')] }, false, captureThreadRead(id));
  const page: ThreadItemListReply = { projection: projection(), items: [answer('stale', 'Old A')], turns: [], segments: [], has_older: false, has_newer: false };
  old.resolve(Response.json(page)); await refusal;
  expect(threadViewOf(id).order).toEqual(['fresh']);
});

it('scopes positioned item/output/count/outline/history/raw/export requests and their query keys', async () => {
  const scope = projection(B, A, 7, false); const { id } = installed(scope);
  const transport = http((request) => {
    const path = new URL(request.url).pathname;
    if (path.endsWith('/output')) return Promise.resolve(new Response('Frozen output'));
    if (path.endsWith('/export')) return Promise.resolve(new Response('Frozen export', { headers: { 'Content-Disposition': 'attachment; filename="view.md"' } }));
    if (path.endsWith('/catchup')) {
      const page: Catchup = { projection: scope, since: 0, seq: 10, items: 0, turns_completed: 0, turns_failed: 0,
        commands: 0, files_changed: 0, approvals_pending: 0, errors: 0, milestones: [], announcements: [] };
      return Promise.resolve(Response.json(page));
    }
    if (path.endsWith('/outline')) return Promise.resolve(Response.json({ projection: scope, entries: [], next_before: null, seq: 10 }));
    if (path.endsWith('/segments')) return Promise.resolve(Response.json({ projection: scope, segments: [], next_before: null, seq: 10 }));
    if (path.endsWith('/turns')) return Promise.resolve(Response.json({ projection: scope, turns: [], segments: [], next_before: null, seq: 10 }));
    if (path.endsWith('/raw')) {
      const page: ThreadRawListReply = { projection: scope, events: [], has_older: false };
      return Promise.resolve(Response.json(page));
    }
    return Promise.resolve(Response.json(answer('item', 'Frozen item', A)));
  });
  await fetchItem(id, 'item');
  expect(await fetchOutput(id, 'item', 0, 1024)).toBe('Frozen output');
  const counts = catchupQuery(id, 0);
  if (typeof counts.queryFn !== 'function') throw new Error('Count query missing');
  await counts.queryFn({ client: queryClient, queryKey: counts.queryKey, signal: new AbortController().signal, meta: undefined });
  const outline = outlineQuery(id);
  if (typeof outline.queryFn !== 'function') throw new Error('Outline query missing');
  await outline.queryFn({ client: queryClient, queryKey: outline.queryKey, signal: new AbortController().signal, meta: undefined, pageParam: null, direction: 'forward' });
  const segments = segmentsQuery(id);
  if (typeof segments.queryFn !== 'function') throw new Error('Segments query missing');
  await segments.queryFn({ client: queryClient, queryKey: segments.queryKey, signal: new AbortController().signal, meta: undefined, pageParam: null, direction: 'forward' });
  const turns = turnsQuery(id);
  if (typeof turns.queryFn !== 'function') throw new Error('Turns query missing');
  await turns.queryFn({ client: queryClient, queryKey: turns.queryKey, signal: new AbortController().signal, meta: undefined, pageParam: null, direction: 'forward' });
  const raw = await readAdmittedRaw(id, 'segment');
  const quoted = viewedItemQuery(id, 'item');
  if (typeof quoted.queryFn !== 'function') throw new Error('Quote query missing');
  await quoted.queryFn({ client: queryClient, queryKey: quoted.queryKey, signal: new AbortController().signal, meta: undefined });
  const file = await exportViewedThreadFile(id, { format: 'markdown', detail: 'normal', include: [] });
  expect(file.name).toBe('view.md');
  for (const request of transport.calls) {
    const url = new URL(request.url);
    expect(url.searchParams.get('continuation_id')).toBe(B);
    expect(url.searchParams.get('projection_revision')).toBe('7');
  }
  const oldKeys = [counts.queryKey, outline.queryKey, segments.queryKey, turns.queryKey, raw.queryKey, quoted.queryKey];
  selectThreadContinuation(id, A); selectThreadContinuation(id, B);
  expect(outlineQuery(id).queryKey).not.toEqual(oldKeys[1]);
  const before = transport.calls.length;
  await expect(quoted.queryFn({ client: queryClient, queryKey: quoted.queryKey, signal: new AbortController().signal, meta: undefined })).rejects.toMatchObject({ name: 'AbortError' });
  expect(transport.calls).toHaveLength(before);
});

it('keeps following active by omitting continuation_id while retaining the projection revision', async () => {
  const { id } = installed();
  const transport = http(() => Promise.resolve(Response.json(answer('item', 'Active item'))));
  await fetchItem(id, 'item');
  const url = new URL(transport.calls[0]?.url ?? 'http://invalid');
  expect(url.searchParams.has('continuation_id')).toBe(false);
  expect(url.searchParams.get('projection_revision')).toBe('1');
});

it('sends a scoped read ack for an inactive view without an activation or input act', async () => {
  const scope = projection(B, A, 3, false); const { id } = installed(scope);
  const transport = http(() => Promise.resolve(Response.json({ projection: scope, last_read_seq: 9 })));
  expect(await markRead(id, 9)).toBe(9);
  const request = transport.calls[0];
  if (request === undefined) throw new Error('Read request absent');
  const body: unknown = await request.json();
  expect(body).toEqual({ seq: 9, continuation_id: B, projection_revision: 3 });
  expect(threadViewOf(id).thread?.my_last_read_seq).toBe(9);
  expect(transport.calls).toHaveLength(1);
});

it('refuses an execution act from explicit browsing and fences a completed active receipt after a later switch', async () => {
  const { id } = installed(projection(B, A, 2, false));
  const row = queued(0); const reply: ThreadInputCreateReply = { input: row, seq: 11 };
  const transport = http(() => Promise.resolve(Response.json(reply)));
  await expect(sendInput(id, { content: [{ type: 'text', text: 'Do not submit' }], delivery: 'queue', clientId: 'not-sent' })).rejects.toMatchObject({ name: 'AbortError' });
  expect(transport.calls).toHaveLength(0);
  selectThreadContinuation(id, null);
  threadWriter.reset(id, snapshot(), false, captureThreadRead(id));
  const received = await sendInput(id, { content: [{ type: 'text', text: 'Active act' }], delivery: 'queue', clientId: 'exact-active-body' });
  selectThreadContinuation(id, B); selectThreadContinuation(id, null);
  threadWriter.reset(id, snapshot(), false, captureThreadRead(id));
  threadWriter.apply(id, { inputs: [received.input] });
  expect(threadViewOf(id).inputs.has(row.id)).toBe(false);
  expect(transport.calls).toHaveLength(1);
});

it('loads only the next100 pending/failed queue rows on demand and never walks delivered history', async () => {
  const scope = projection(B, A, 2, false); const { id } = installed(scope);
  const initial = Array.from({ length: 100 }, (_, index) => queued(index));
  threadWriter.reset(id, { ...snapshot(scope), inputs: initial, inputs_next_after: inputId(99), inputs_queue_revision: 5 }, false, captureThreadRead(id));
  const page: ThreadInputPage = { projection: scope, inputs: [queued(100), { ...queued(101), state: 'failed' }], next_after: null, queue_revision: 5, seq: 12 };
  const transport = http(() => Promise.resolve(Response.json(page)));
  expect(transport.calls).toHaveLength(0);
  await loadNextThreadInputs(id); await loadNextThreadInputs(id);
  expect(transport.calls).toHaveLength(1);
  const url = new URL(transport.calls[0]?.url ?? 'http://invalid');
  expect(url.pathname).toBe(`/api/v1/threads/${id}/inputs`);
  expect(url.searchParams.get('after')).toBe(inputId(99));
  expect(url.searchParams.get('queue_revision')).toBe('5');
  expect(url.searchParams.get('limit')).toBe('100');
  expect([...threadViewOf(id).inputs.values()].some((row) => row.state === 'delivered')).toBe(false);
  expect(threadViewOf(id).inputs.size).toBe(102);
  expect(threadViewOf(id).queuePage.nextAfter).toBeNull();
});

it('an old queue read cannot append reordered rows or clear the newer page request owner', async () => {
  const scope = projection(B, A, 2, false); const { id } = installed(scope);
  threadWriter.reset(id, { ...snapshot(scope), inputs: [queued(0)], inputs_next_after: inputId(0), inputs_queue_revision: 1 }, false, captureThreadRead(id));
  const old = deferred<Response>(), next = deferred<Response>();
  const transport = http((request) => new URL(request.url).searchParams.get('queue_revision') === '1' ? old.promise : next.promise);
  const pending = loadNextThreadInputs(id); await transport.requests.next();
  await loadNextThreadInputs(id); expect(transport.calls).toHaveLength(1);
  threadWriter.apply(id, { inputs_queue_revision: 2, inputs_next_after: inputId(0) }, captureThreadRead(id));
  const current = loadNextThreadInputs(id); await transport.requests.next();
  const refusal = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  old.resolve(Response.json({ projection: scope, inputs: [queued(1)], next_after: null, queue_revision: 1, seq: 11 })); await refusal;
  expect(threadViewOf(id).inputs.has(inputId(1))).toBe(false);
  expect(threadViewOf(id).queuePage.loading).toBe(true);
  next.resolve(Response.json({ projection: scope, inputs: [queued(2)], next_after: null, queue_revision: 2, seq: 12 })); await current;
  expect(threadViewOf(id).inputs.has(inputId(2))).toBe(true);
  expect(threadViewOf(id).queuePage.loading).toBe(false);
});

it('fences a late read even when its original view has not changed but the actual reader did', async () => {
  const { id } = installed(); const waiting = deferred<Response>();
  const transport = http(() => waiting.promise);
  const pending = fetchItem(id, 'item'); await transport.requests.next();
  const reader = client.session.getState().readerVersion;
  client.session.setState({ readerVersion: (reader ?? 0) + 1 });
  const refusal = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  waiting.resolve(Response.json(answer('item', 'Old reader'))); await refusal;
  client.session.setState({ readerVersion: reader });
  expect(threadViewOf(id).items.has('item')).toBe(false);
});

it('an active pin receipt updates shared metadata without replacing a frozen inherited body', async () => {
  const scope = projection(B, B, 4); const { id } = installed(scope);
  const inherited = answer('prefix', 'Frozen prefix', A);
  threadWriter.apply(id, { items: [inherited] }, captureThreadRead(id));
  const currentOwner = { ...answer('prefix', 'Former owner live body', A, 99), chat_refs: 4 };
  const transport = http(() => Promise.resolve(Response.json(currentOwner)));
  const returned = await pinItem(id, 'prefix', false);
  threadWriter.apply(id, { items: [returned] });
  const held = threadViewOf(id).items.get('prefix');
  expect(held?.kind === 'agent_message' ? held.payload.text : '').toBe('Frozen prefix');
  expect(held?.chat_refs).toBe(4); expect(held?.seq_updated).toBe(99);
  expect(transport.calls).toHaveLength(1);
});

it('constructs scoped query options without mutating store state during render', () => {
  const id = snapshot().thread.id;
  const before = threadViewOf(id);
  catchupQuery(id, 0); outlineQuery(id); turnsQuery(id); segmentsQuery(id); viewedItemQuery(id, 'item'); viewedRawFramesQuery(id, 'segment');
  expect(threadViewOf(id)).toBe(before);
  expect(threadViewOf(id).read).toBeNull();
});

it('primes the native outline/settings/draft before publishing the opening thread to controls', async () => {
  const sync = snapshot(); const id = sync.thread.id;
  const response = opening(sync);
  const transport = http(() => Promise.resolve(Response.json(response)));
  const { useThreadStore } = await import('@/realtime/stores/thread');
  let published = 0;
  const stop = useThreadStore.subscribe(() => {
    if (threadViewOf(id).thread === null) return;
    published += 1;
    expect(queryClient.getQueryData(threadKeys.outline(id))).toMatchObject({ pages: [response.outline], pageParams: [null] });
    expect(queryClient.getQueryState(threadKeys.draft(id))).toBeDefined();
  });
  try { await fetchThreadSync(id, null); }
  finally { stop(); }
  expect(published).toBeGreaterThan(0); expect(transport.calls).toHaveLength(1);
});

it('same-order live queue metadata cannot rewind an explicitly paged continuation cursor', async () => {
  const scope = projection(B, A, 2, false); const { id } = installed(scope);
  const first = Array.from({ length: 100 }, (_, index) => queued(index));
  threadWriter.reset(id, { ...snapshot(scope), inputs: first, inputs_next_after: inputId(99), inputs_queue_revision: 8 }, false, captureThreadRead(id));
  const httpCalls = http((request) => {
    const after = new URL(request.url).searchParams.get('after');
    const inputs = after === inputId(99) ? Array.from({ length: 100 }, (_, index) => queued(index + 100)) : [queued(200)];
    const page: ThreadInputPage = { projection: scope, inputs, next_after: after === inputId(99) ? inputId(199) : null, queue_revision: 8, seq: 20 };
    return Promise.resolve(Response.json(page));
  });
  await loadNextThreadInputs(id);
  threadWriter.apply(id, { projection: scope, inputs_queue_revision: 8, inputs_next_after: inputId(99) }, captureThreadRead(id));
  await loadNextThreadInputs(id);
  expect(httpCalls.calls.map((request) => new URL(request.url).searchParams.get('after'))).toEqual([inputId(99),inputId(199)]);
  expect(threadViewOf(id).inputs.size).toBe(201); expect(threadViewOf(id).queuePage.nextAfter).toBeNull();
});


it('bounds live unpaged inputs to the native head and requires an explicit head restart after the boundary changes', async () => {
  const scope = projection(B, A, 2, false); const { id } = installed(scope);
  const initial = Array.from({ length: 100 }, (_, index) => queued(index));
  threadWriter.reset(id, { ...snapshot(scope), inputs: initial, inputs_next_after: inputId(99), inputs_queue_revision: 1 }, false, captureThreadRead(id));
  const transport = http(() => Promise.resolve(Response.json({ projection: scope, inputs: Array.from({ length: 100 }, (_, index) => queued(index + 1)),
    next_after: inputId(100), queue_revision: 2, seq: 20 })));
  // These are native changed rows, not a complete new first page. Existing pending #100
  // has not changed and is absent; later live additions cannot turn into an implicit page walk.
  threadWriter.apply(id, { inputs: [{ ...queued(0), state: 'cancelled', seq_updated: 20 },
    ...Array.from({ length: 150 }, (_, index) => queued(index + 101))], inputs_queue_revision: 2,
    inputs_next_after: inputId(100) }, captureThreadRead(id));
  expect(threadViewOf(id).inputs.size).toBeLessThanOrEqual(100);
  expect(threadViewOf(id).queuePage.restartRequired).toBe(true);
  expect(transport.calls).toHaveLength(0);
  await loadNextThreadInputs(id);
  const request = transport.calls[0]; if (request === undefined) throw new Error('Queue restart absent');
  const url = new URL(request.url);
  expect(url.searchParams.has('after')).toBe(false); expect(url.searchParams.has('queue_revision')).toBe(false);
  expect(url.searchParams.get('limit')).toBe('100');
  expect([...threadViewOf(id).inputs.keys()]).toEqual(Array.from({ length: 100 }, (_, index) => inputId(index + 1)));
  expect(threadViewOf(id).queuePage.restartRequired).toBe(false);
  expect(threadViewOf(id).queuePage.nextAfter).toBe(inputId(100));
  expect(transport.calls).toHaveLength(1);
});

it('keeps actual timeline and current-turn input references when an explicitly paged queue is restarted', async () => {
  const scope = projection(B, B, 2); const { id } = installed(scope);
  const current = turn('current', { ordinal: 1, inputId: inputId(101), state: 'running' });
  const delivered = { ...queued(100), state: 'delivered' as const, turn_id: current.id, seq_updated: 20 };
  const delivering = { ...queued(101), state: 'delivering' as const, turn_id: current.id, seq_updated: 20 };
  const capture = captureThreadRead(id);
  const head = Array.from({ length: 100 }, (_, index) => queued(index));
  threadWriter.apply(id, { inputs: head, inputs_queue_revision: 1, inputs_next_after: inputId(99) },capture);
  const request = {};
  threadWriter.queueLoading(id,capture,request);
  threadWriter.queueLoaded(id,{ projection: scope, inputs: [queued(100),queued(101)], next_after: null, queue_revision: 1, seq: 10 },capture,false,request);
  threadWriter.apply(id, { thread: { ...snapshot(scope).thread, seq: 20, current_turn_id: current.id }, turns: [current],
    items: [userMessage('retained-prompt', 'Delivered', { author: ADA, inputId: delivered.id })],
    inputs: [...head.map((row) => ({ ...row, state: 'cancelled' as const, seq_updated: 20 })),delivered,delivering],
    inputs_queue_revision: 2, inputs_next_after: null },capture);
  expect(threadViewOf(id).inputs.get(delivered.id)).toEqual(delivered);
  expect(threadViewOf(id).inputs.get(delivering.id)).toEqual(delivering);
  const transport = http(() => Promise.resolve(Response.json({ projection: scope, inputs: [delivering], next_after: null, queue_revision: 2, seq: 21 })));
  await loadNextThreadInputs(id);
  expect(threadViewOf(id).inputs.get(delivered.id)).toEqual(delivered);
  expect(threadViewOf(id).inputs.get(delivering.id)).toEqual(delivering);
  expect(threadViewOf(id).inputs.has(inputId(0))).toBe(false); expect(transport.calls).toHaveLength(1);
});

it('refuses execution while a known active read is reloading, before any native request', async () => {
  const { id } = installed();
  threadWriter.beginRead(id,client.session.getState().readerVersion);
  const transport = http(() => Promise.reject(new Error('Loading history cannot submit')));
  await expect(sendInput(id,{ content: [{ type: 'text', text: 'No work' }], delivery: 'queue', clientId: 'not-sent-loading' }))
    .rejects.toMatchObject({ name: 'AbortError' });
  await expect(fetchItems(id,{ before: 5 })).rejects.toMatchObject({ name: 'AbortError' });
  expect(transport.calls).toHaveLength(0);
});
