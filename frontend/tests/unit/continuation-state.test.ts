/** Actual store consumers cannot merge another history, including repeated away/back and late receipts. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { captureThreadRead, selectThreadContinuation, stampThreadRows, threadReadQueryKey } from '@/api/threadRead';
import { applyRows, EMPTY_VIEW, flushDeltas, GAP_READ_AGAIN_MS, setGapHandler, setThreadReadChangeHandler, setThreadReadRetainer,
  threadReadOnly, threadViewOf, threadWriter, waitingInputs } from '@/realtime/stores/thread';
import { stampThreadReceipt } from '@/realtime/threadScope';
import { threadInput, turn, userMessage } from '../doubles/thread-rows';
import { me, userRef } from '../doubles/fixtures';
import { A, B, C, answer, projection, snapshot, summary, viewedPrompt } from './continuation-fixtures';

const ADA = userRef(me(1, 'ada', 'Ada Lovelace'));

function install(scope = projection(), text = 'First') {
  const sync = snapshot(scope); const id = sync.thread.id;
  if (!scope.follows_active) selectThreadContinuation(id, scope.viewed_continuation_id);
  const capture = captureThreadRead(id);
  const accepted = threadWriter.reset(id, { ...sync, items: [answer('answer', text, scope.viewed_continuation_id)] }, false, capture);
  if (accepted === null) throw new Error('Native fixture projection was not admitted');
  return { id, capture: accepted };
}

beforeEach(() => { threadWriter.clear(); setThreadReadChangeHandler(() => undefined); setThreadReadRetainer(() => undefined); });
afterEach(() => { flushDeltas(); threadWriter.clear(); vi.useRealTimers(); });

it('keeps same-projection turn references, but drops every old branch reference and marker on selection', () => {
  const { id, capture } = install();
  const old = turn('old', { ordinal: 1, state: 'completed', inputId: 'old-input' });
  threadWriter.apply(id, { turns: [old] }, capture);
  threadWriter.reset(id, { ...snapshot(), items: [answer('new', 'New')] }, true, capture);
  expect(threadViewOf(id).turns.get(old.id)).toEqual(old);
  const key = threadReadQueryKey(id, ['thread-turns', id]);
  const metadata = threadViewOf(id).thread;
  selectThreadContinuation(id, B);
  expect(threadViewOf(id).thread).toBe(metadata);
  expect(threadViewOf(id).items.size).toBe(0);
  expect(threadViewOf(id).turns.size).toBe(0);
  expect(threadViewOf(id).segments).toEqual([]);
  expect(threadViewOf(id).openedReadSeq).toBeNull();
  expect(threadReadOnly(threadViewOf(id))).toBe(true);
  expect(threadReadQueryKey(id, ['thread-turns', id])).not.toEqual(key);
});

it('reads a growing answer again while the answer it was given is still behind the pieces that wait', () => {
  vi.useFakeTimers();
  const asked: string[] = [];
  setGapHandler((_thread, itemId) => { asked.push(itemId); });
  try {
    const { id, capture } = install(projection(), 'First');
    // A piece begins beyond what is held: the answer is read again.
    threadWriter.delta(id, { item_id: 'answer', field: 'text', offset: 10, len: 5, data: ' more' }, capture);
    flushDeltas();
    expect(asked).toEqual(['answer']);
    // The server had written only part of what it had sent: still behind, it is read again a moment later.
    threadWriter.itemFetched(id, stampThreadReceipt(answer('answer', 'First pa', A, 11), capture), capture);
    expect(asked).toEqual(['answer']);
    vi.advanceTimersByTime(GAP_READ_AGAIN_MS);
    expect(asked).toEqual(['answer', 'answer']);
    // Now it reaches the waiting piece, which follows at once; nothing more is asked.
    threadWriter.itemFetched(id, stampThreadReceipt(answer('answer', 'First part', A, 12), capture), capture);
    vi.advanceTimersByTime(GAP_READ_AGAIN_MS);
    expect(asked).toEqual(['answer', 'answer']);
    const held = threadViewOf(id).items.get('answer');
    expect(held?.kind === 'agent_message' ? held.payload.text : '').toBe('First part more');
  } finally {
    setGapHandler(() => undefined);
  }
});

it('rejects an old completed page, gap receipt and queued delta after A -> B -> A', () => {
  const { id, capture } = install();
  const page = stampThreadRows({ items: [answer('old-page', 'Old page')] }, capture);
  const gap = stampThreadReceipt(answer('answer', 'Wrong gap result', A, 999), capture);
  threadWriter.delta(id, { item_id: 'answer', field: 'text', offset: 5, len: 6, data: ' wrong' }, capture);
  selectThreadContinuation(id, B); selectThreadContinuation(id, A);
  const current = captureThreadRead(id);
  const accepted = threadWriter.reset(id, { ...snapshot(projection(A, B, 1, false)), items: [answer('answer', 'Fresh')] }, false, current);
  expect(accepted).not.toBeNull();
  threadWriter.older(id, page.items, true);
  threadWriter.itemFetched(id, gap);
  threadWriter.apply(id, { inputs: [stampThreadReceipt(threadInput('late', 'Late', { author: ADA, position: '1' }), capture)] });
  flushDeltas();
  const held = threadViewOf(id).items.get('answer');
  expect(held?.kind === 'agent_message' ? held.payload.text : '').toBe('Fresh');
  expect(threadViewOf(id).items.has('old-page')).toBe(false);
  expect(threadViewOf(id).inputs.has('late')).toBe(false);
});

it('rejects an old lifetime after eviction even when selection, thread and numeric revisions repeat', () => {
  const { id, capture } = install();
  const oldKey = threadReadQueryKey(id, ['thread-outline', id]);
  const old = stampThreadReceipt(answer('retired', 'Old'), capture);
  threadWriter.evict(id);
  expect(capture.scope.controller.signal.aborted).toBe(true);
  const fresh = install();
  expect(fresh.capture.scope.controller.signal.aborted).toBe(false);
  expect(threadReadQueryKey(id, ['thread-outline', id])).not.toEqual(oldKey);
  threadWriter.older(id, [old], false);
  expect(threadViewOf(id).items.has(old.id)).toBe(false);
});

it('never appends a live delta into an inherited frozen prefix, including delta-before-row replay', () => {
  const { id, capture } = install(projection(B, C, 2, false));
  threadWriter.apply(id, { items: [stampThreadReceipt(answer('prefix', 'Frozen', A), capture),
    stampThreadReceipt({ ...answer('unknown-prefix', 'Frozen'), continuation_id: null }, capture)] }, capture);
  threadWriter.delta(id, { item_id: 'unknown-prefix', field: 'text', offset: 6, len: 5, data: ' live' }, capture);
  threadWriter.delta(id, { item_id: 'prefix', field: 'text', offset: 6, len: 5, data: ' live' }, capture);
  threadWriter.delta(id, { item_id: 'late-prefix', field: 'text', offset: 6, len: 5, data: ' live' }, capture);
  flushDeltas();
  threadWriter.apply(id, { items: [stampThreadReceipt(answer('late-prefix', 'Frozen', A), capture)] }, capture);
  for (const key of ['prefix','late-prefix','unknown-prefix']) {
    const row = threadViewOf(id).items.get(key);
    expect(row?.kind === 'agent_message' ? row.payload.text : '').toBe('Frozen');
    expect(threadViewOf(id).held.has(key)).toBe(false);
  }
});

it('keeps exact viewed preparation status after reload without inventing a delivered message or using active Thread.job', () => {
  const scope = projection(B, A, 2, false);
  const { id, capture } = install(scope);
  const pending = threadInput('edited-input', 'Revised words', { author: ADA, position: '1' });
  const native = { ...snapshot(scope), inputs: [pending], viewed_prompt: viewedPrompt('preparing') };
  threadWriter.reset(id, native, false, capture);
  const before = threadViewOf(id).read;
  expect(threadViewOf(id).viewedPrompt?.status).toBe('preparing');
  expect(waitingInputs(threadViewOf(id)).map((row) => row.id)).toEqual([pending.id]);
  threadWriter.apply(id, { projection: scope, viewed_prompt: viewedPrompt('failed') }, capture);
  expect(threadViewOf(id).viewedPrompt).toMatchObject({ status: 'failed', group_id: 'synthetic-group', total: 2,
    job: { state: 'failed' }, input_id: pending.id });
  expect(threadViewOf(id).read?.epoch).toBe(before?.epoch);
  expect(threadViewOf(id).items.size).toBe(0);
  expect(threadReadOnly(threadViewOf(id))).toBe(true);
  threadWriter.clear();
  selectThreadContinuation(id, B);
  const refreshed = captureThreadRead(id);
  threadWriter.reset(id, { ...native, viewed_prompt: viewedPrompt('failed') }, false, refreshed);
  expect(threadViewOf(id).viewedPrompt?.status).toBe('failed');
  expect(threadViewOf(id).inputs.get(pending.id)?.state).toBe('pending');
});

it('bounds prompt summaries to actual held input references and keeps summary progress separate from projection identity', () => {
  const { id, capture } = install();
  const item = userMessage('prompt', 'Words', { author: ADA, inputId: 'visible' });
  const queue = threadInput('queued', 'Later', { author: ADA, position: '1' });
  const orphans = Array.from({ length: 100 }, (_, index) => summary(`not-held-${index}`));
  threadWriter.apply(id, { items: [item], inputs: [queue], prompt_summaries: [summary('visible'), summary('queued'), ...orphans] }, capture);
  expect([...threadViewOf(id).promptSummaries.keys()].sort()).toEqual(['queued','visible']);
  const read = threadViewOf(id).read;
  threadWriter.apply(id, { prompt_summaries: [{ ...summary('visible'), total: 3 }] }, capture);
  expect(threadViewOf(id).promptSummaries.get('visible')?.total).toBe(3);
  expect(threadViewOf(id).read).toBe(read);
  // This is current native metadata, not a client-invented monotonic count/capability.
  threadWriter.apply(id, { prompt_summaries: [{ ...summary('visible'), total: 1 }] }, capture);
  expect(threadViewOf(id).promptSummaries.get('visible')?.total).toBe(1);
});

it('scopes read acknowledgments and never applies a legacy or other continuation marker to an inactive view', () => {
  const { id } = install(projection(B, A, 2, false));
  threadWriter.readState(id, null, 100);
  threadWriter.readState(id, A, 100);
  expect(threadViewOf(id).thread?.my_last_read_seq).toBe(0);
  threadWriter.readState(id, B, 8);
  expect(threadViewOf(id).thread?.my_last_read_seq).toBe(8);
  expect(threadViewOf(id).openedReadSeq).toBe(0);
  threadWriter.readState(id, B, 4);
  expect(threadViewOf(id).thread?.my_last_read_seq).toBe(8);
});

it('retains the original legacy row/delta behavior when optional continuation DTOs are omitted', () => {
  const sync = snapshot(null); const id = sync.thread.id;
  threadWriter.reset(id, { ...sync, items: [answer('legacy', 'Hello')] }, false);
  threadWriter.delta(id, { item_id: 'legacy', field: 'text', offset: 5, len: 1, data: '!' });
  flushDeltas();
  const held = threadViewOf(id).items.get('legacy');
  expect(held?.kind === 'agent_message' ? held.payload.text : '').toBe('Hello!');
  expect(applyRows(EMPTY_VIEW, { items: [held].filter((row) => row !== undefined) }).order).toEqual(['legacy']);
});

it('historical page metadata cannot replace the ordered live summary of an already held prompt', () => {
  const { id, capture } = install();
  const item = userMessage('prompt', 'Words', { author: ADA, inputId: 'known' });
  threadWriter.apply(id, { items: [item], prompt_summaries: [{ ...summary('known'), total: 3 }] }, capture);
  const oldPage = stampThreadRows({ items: [{ ...item, seq_updated: 99 }], prompt_summaries: [summary('known')] }, capture);
  threadWriter.older(id, oldPage.items, false);
  expect(threadViewOf(id).promptSummaries.get('known')?.total).toBe(3);
});


it('keeps a selected history read marker while accepting current global title and permission metadata', () => {
  const { id } = install(projection(B,A,2,false));
  threadWriter.readState(id,B,7);
  const native = snapshot().thread;
  threadWriter.apply(id,{ thread: { ...native, seq: 30, name: 'Current title', my_last_read_seq: 99 } });
  const view = threadViewOf(id);
  expect(view.thread?.name).toBe('Current title'); expect(view.thread?.can).toEqual(native.can);
  expect(view.thread?.my_last_read_seq).toBe(7); expect(view.openedReadSeq).toBe(0);
  expect(view.read?.projection?.viewed_continuation_id).toBe(B);
});
