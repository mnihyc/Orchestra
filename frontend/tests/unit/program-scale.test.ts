/** Large program histories and paste previews have bounded work and retained state. */
import { expect, it, vi } from 'vitest';
import { countTextCharacters, countTextCharactersAsync, estimateTokenCounts } from '@/lib/tokenEstimate';
import { INLINE_PASTE_MAX_CHARS, pasteWouldScroll, pastedPartTokens } from '@/lib/pastedText';
import { TextPreviewCache, TEXT_PREVIEW_CACHE_BYTES, TEXT_PREVIEW_CACHE_ENTRIES } from '@/lib/textPreviewCache';
import { newOutputCounts, observeOutput, OUTPUT_COUNT_SNAPSHOT_CHARS } from '@/realtime/outputCounts';
import { applyDeltas, applyRows, EMPTY_VIEW, viewFromRows } from '@/realtime/stores/thread';
import { agentMessage } from '../doubles/thread-rows';
import { statusContext } from './status-items-fixtures';

const estimate = { ascii_prose_x100: 400, ascii_structured_x100: 350, other_x100: 100, image_tokens: 1000, measured: false };

it('keeps more than ten billion tokens numeric without reading a paste body', () => {
  const part = { type: 'pasted_text' as const, blob: 'blob', path: 'paste.txt', name: 'paste.txt', size: 40_000_000_004, characters: 40_000_000_004, ascii_characters: 40_000_000_000 };
  expect(pastedPartTokens(part, estimate)).toBe(10_000_000_004);
  expect(estimateTokenCounts(40_000_000_000, 4, estimate)).toBe(10_000_000_004);
  expect(estimateTokenCounts(Number.MAX_SAFE_INTEGER, 0, estimate)).toBeNull();
});

it('keeps cache entries and bytes bounded through a million previews', () => {
  const cache = new TextPreviewCache();
  let entriesHeld = 0, bytesHeld = 0;
  const text = 'x'.repeat(1024);
  for (let row = 0; row < 1_000_000; row += 1) {
    cache.set(String(row), text);
    entriesHeld = Math.max(entriesHeld, cache.size); bytesHeld = Math.max(bytesHeld, cache.bytes);
  }
  expect(entriesHeld).toBe(TEXT_PREVIEW_CACHE_ENTRIES);
  expect(bytesHeld).toBeLessThanOrEqual(TEXT_PREVIEW_CACHE_BYTES);
  expect(cache.get('0')).toBeUndefined();
  expect(cache.get('999999')).toBe(text);
  const large = '文'.repeat(TEXT_PREVIEW_CACHE_BYTES / 4);
  cache.set('large', large); cache.set('second', large); cache.get('large'); cache.set('third', large);
  expect(cache.bytes).toBe(TEXT_PREVIEW_CACHE_BYTES);
  expect(cache.get('second')).toBeUndefined();
  expect(cache.get('large')).toBe(large);
  cache.set('oversized', large + large + 'x');
  expect(cache.get('oversized')).toBeUndefined();
  cache.clear(); expect(cache.size).toBe(0); expect(cache.bytes).toBe(0);
});

it('counts a million Unicode characters cooperatively and permits cancellation', async () => {
  const text = 'a文😊'.repeat(333_334);
  const controller = new AbortController();
  const pending = countTextCharactersAsync(text, controller.signal);
  let yielded = false;
  queueMicrotask(() => { yielded = true; });
  expect(await pending).toEqual({ ascii: 333_334, other: 666_668 });
  expect(yielded).toBe(true);
  expect(countTextCharacters('a\ud800b\udc00😊')).toEqual({ ascii: 2, other: 3 });
  const cancelled = countTextCharactersAsync(text, controller.signal);
  controller.abort();
  await expect(cancelled).rejects.toMatchObject({ name: 'AbortError' });
});

it('folds a large paste without a layout read or a hidden copy of its text', () => {
  const element = document.createElement('textarea');
  const readStyle = vi.spyOn(window, 'getComputedStyle');
  const clone = vi.spyOn(element, 'cloneNode');
  expect(pasteWouldScroll(element, 'a'.repeat(INLINE_PASTE_MAX_CHARS * 10), 10)).toBe(true);
  expect(readStyle).not.toHaveBeenCalled(); expect(clone).not.toHaveBeenCalled();
  readStyle.mockRestore();
});

it('updates output counters once across duplicates, overlaps, replacement and the next turn', () => {
  const context = statusContext(), thread = context.thread;
  const first = { ...agentMessage('answer', 'a😊', { turn: thread.current_turn_id, status: 'in_progress' }), seq_created: 1, seq_updated: 1 };
  let view = viewFromRows(EMPTY_VIEW, { thread, items: [first] }, false);
  expect(view.outputCounts.counts).toEqual({ ascii: 1, other: 1 });
  const delta = { item_id: first.id, field: 'text' as const, offset: 1, len: 3, data: '😊文b' };
  view = applyDeltas(view, [delta, delta]).view;
  expect(view.outputCounts.counts).toEqual({ ascii: 2, other: 2 });
  view = applyRows(view, { items: [{ ...first, seq_updated: 2 }] });
  expect(view.outputCounts.counts).toEqual({ ascii: 2, other: 2 });
  const final = { ...agentMessage(first.id, 'done', { turn: thread.current_turn_id }), seq_created: 1, seq_updated: 3 };
  view = applyRows(view, { items: [final] });
  expect(view.outputCounts.counts).toEqual({ ascii: 4, other: 0 });
  const next = { ...thread, seq: thread.seq + 1, current_turn_id: 'next' };
  view = applyRows(view, { thread: next, items: [{ ...agentMessage('next-answer', '中文', { turn: 'next' }), seq_created: 4, seq_updated: 4 }] });
  expect(view.outputCounts.counts).toEqual({ ascii: 0, other: 2 });
});

it('counts only an appended suffix and retains constant-size totals beyond ten billion tokens', () => {
  const before = agentMessage('answer', 'a', { turn: 'active' });
  const initial = observeOutput(newOutputCounts('active'), undefined, before);
  const large = { ...initial, counts: { ascii: 40_000_000_000, other: 0 } };
  const after = agentMessage('answer', 'a文', { turn: 'active' });
  // A streamed update must use the piece, even when touching a joined string would flatten it.
  Object.defineProperty(after.payload, 'text', { get: () => { throw new Error('The joined body must not be read'); } });
  const reads = vi.spyOn(String.prototype, 'charCodeAt');
  const updated = observeOutput(large, before, after, '文');
  const count = reads.mock.calls.length; reads.mockRestore();
  expect(count).toBeLessThanOrEqual(4);
  expect(updated.counts).toEqual({ ascii: 40_000_000_000, other: 1 });
  expect(Object.keys(updated)).toHaveLength(3);
  expect(estimateTokenCounts(updated.counts?.ascii ?? 0, updated.counts?.other ?? 0, estimate)).toBe(10_000_000_001);
  const huge = agentMessage('snapshot', 'a'.repeat(OUTPUT_COUNT_SNAPSHOT_CHARS * 10), { turn: 'active' });
  const scan = vi.spyOn(String.prototype, 'charCodeAt');
  const unknown = observeOutput(newOutputCounts('active'), undefined, huge);
  const scanned = scan.mock.calls.length; scan.mockRestore();
  expect(unknown.counts).toBeNull(); expect(scanned).toBe(0);
  const inactive = newOutputCounts(null);
  const historical = agentMessage('historical', 'body', { turn: null });
  Object.defineProperty(historical.payload, 'text', { get: () => { throw new Error('Inactive output must not be read'); } });
  expect(observeOutput(inactive, undefined, historical)).toBe(inactive);
});

it('keeps a counted long stream when an older persisted row arrives behind it', () => {
  const thread = statusContext().thread;
  const first = { ...agentMessage('long-answer', 'a'.repeat(OUTPUT_COUNT_SNAPSHOT_CHARS), { turn: thread.current_turn_id, status: 'in_progress' }), seq_created: 1, seq_updated: 1 };
  let view = viewFromRows(EMPTY_VIEW, { thread, items: [first] }, false);
  view = applyDeltas(view, [{ item_id: first.id, field: 'text', offset: OUTPUT_COUNT_SNAPSHOT_CHARS, len: 1, data: '文' }]).view;
  view = applyRows(view, { items: [{ ...first, seq_updated: 2 }] });
  expect(view.outputCounts.counts).toEqual({ ascii: OUTPUT_COUNT_SNAPSHOT_CHARS, other: 1 });
});

it('keeps counters independent when two projections admit the same row and stream objects', () => {
  const first = agentMessage('answer', 'a', { turn: 'active' });
  const one = observeOutput(newOutputCounts('active'), undefined, first);
  const two = observeOutput(newOutputCounts('active'), undefined, first);
  const after = agentMessage('answer', 'a文', { turn: 'active' });
  expect(observeOutput(one, first, after, '文').counts).toEqual({ ascii: 1, other: 1 });
  expect(observeOutput(two, first, after, '文').counts).toEqual({ ascii: 1, other: 1 });
  expect(one.counts).toEqual({ ascii: 1, other: 0 });
  expect(two.counts).toEqual({ ascii: 1, other: 0 });
});
