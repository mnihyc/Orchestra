/** Pending words belong to immutable input IDs; inactive views cannot execute their queues. */
import { afterEach, expect, it, vi } from 'vitest';
import { captureThreadRead, selectThreadContinuation } from '@/api/threadRead';
import { cancelInput, fetchThreadSync, refreshThreadInputs, resumeQueue, retryInput, updateInput } from '@/api/threads';
import { resend, useComposerStore, type SendingInput } from '@/features/composer/Draft';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead, resultThreadRead, threadReceipt } from '@/realtime/threadScope';
import { threadInput } from '../doubles/thread-rows';
import { deferred, snapshot } from './continuation-fixtures';
import { A, B, C, T, serve } from './thread-read-fixtures';

const route = defaultThreadRead(T), saved = resultThreadRead(T, 'pending-history');
const pending = { ...threadInput('pending-b', 'Retained words', { author: snapshot().thread.owner, position: '1' }), thread_id: T, continuation_id: B };
const active = { ...threadInput('pending-a', 'Selected queue stays intact', { author: snapshot().thread.owner, position: '1' }), thread_id: T, continuation_id: A };
afterEach(() => { threadWriter.clear(); useComposerStore.setState({ sending: {} }); vi.unstubAllGlobals(); });
async function open() {
  await fetchThreadSync(route, null);
  selectThreadContinuation(saved, B); await fetchThreadSync(saved, null);
}

it('edits/cancels only the actual retained input, discards selected neighbors, and refreshes its own queue cut', async () => {
  const revised = { ...pending, content: [{ type: 'text' as const, text: 'Revised retained words' }] };
  const server = serve((request) => {
    if (request.method === 'PATCH') return Promise.resolve(Response.json({ input: revised, inputs: [active] }));
    if (request.method === 'DELETE') return Promise.resolve(Response.json({ ...revised, state: 'cancelled' }));
    if (new URL(request.url).pathname.endsWith('/inputs')) return Promise.resolve(Response.json({
      projection: threadViewOf(saved).read?.projection, inputs: [revised], queue_revision: 9, next_after: null, seq: 21,
    }));
    return undefined;
  });
  await open(); const before = threadViewOf(route);
  const reply = await updateInput(saved, pending.id, { content: revised.content });
  expect(reply.inputs).toEqual([]);
  threadWriter.apply(saved, { inputs: [...reply.inputs, reply.input] });
  expect(threadViewOf(saved).inputs.get(pending.id)?.content).toEqual(revised.content);
  expect(threadViewOf(saved).inputs.has(active.id)).toBe(false);
  const capture = threadReceipt(reply.input); expect(capture).toBeDefined();
  await refreshThreadInputs(capture ?? captureThreadRead(saved));
  expect(threadViewOf(saved).queuePage.revision).toBe(9);
  const removed = await cancelInput(saved, pending.id);
  threadWriter.apply(saved, { inputs: [removed] });
  expect(removed.state).toBe('cancelled'); expect(threadViewOf(route)).toBe(before);
  const mutations = server.calls.filter(request => request.method !== 'GET');
  expect(mutations.map(request => [request.method, new URL(request.url).pathname])).toEqual([
    ['PATCH', `/api/v1/threads/${T}/inputs/${pending.id}`], ['DELETE', `/api/v1/threads/${T}/inputs/${pending.id}`],
  ]);
  expect(await mutations[0]?.clone().json()).toEqual({ content: revised.content });
  const refreshed = server.calls.find(request => new URL(request.url).pathname.endsWith('/inputs'));
  expect(new URL(refreshed?.url ?? 'http://invalid').searchParams.get('continuation_id')).toBe(B);
  expect(threadViewOf(saved).read?.selection).toBe(B);
});

it('refuses inactive delivery, reorder, file retry and Resume before issuing any write', async () => {
  const server = serve(); await open(); const count = server.calls.length;
  const actions: (() => Promise<unknown>)[] = [() => updateInput(saved, pending.id, { delivery: 'queue' }),
    () => updateInput(saved, pending.id, { afterInputId: null }), () => retryInput(saved, pending.id), () => resumeQueue(saved, 1)];
  for (const act of actions) {
    await expect(Promise.resolve().then(act)).rejects.toMatchObject({ name: 'AbortError' });
  }
  expect(server.calls).toHaveLength(count);
});

it('cannot populate a new selection from a late retained-input mutation or refresh its queue', async () => {
  const waiting = deferred<Response>();
  const server = serve(request => request.method === 'PATCH' ? waiting.promise : undefined);
  await open(); const result = updateInput(saved, pending.id, { content: pending.content });
  const refused = expect(result).rejects.toMatchObject({ name: 'AbortError' });
  selectThreadContinuation(saved, C); await fetchThreadSync(saved, null);
  const before = threadViewOf(saved), count = server.calls.length;
  waiting.resolve(Response.json({ input: pending, inputs: [active] })); await refused;
  expect(threadViewOf(saved)).toBe(before); expect(server.calls).toHaveLength(count);
});


it('a retry from another execution history leaves its words and replay identity untouched', async () => {
  const server = serve(); await open();
  const entry: SendingInput = { continuationId: B, clientId: 'retained-replay-key', content: pending.content,
    delivery: 'queue', state: 'not_sent', at: '2026-10-17T00:00:00Z' };
  useComposerStore.setState({ sending: { [T]: [entry] } });
  const before = useComposerStore.getState(), count = server.calls.length;
  resend(route, entry);
  expect(useComposerStore.getState()).toBe(before); expect(server.calls).toHaveLength(count);
});
