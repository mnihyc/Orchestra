/** Resume and compaction repair must preserve message identity, Unicode offsets and current authority. */
import { describe, expect, it } from 'vitest';
import { readAskEvents } from '@/api/ask';
import { acceptsAskEvent, applyAskEvent, newerAskSnapshot } from '@/features/ask/events';
import { fitViewWindow } from '@/features/workbench/ViewWindow';
import type { AskEvent, ContentPart } from '@/generated/api-types';
import { ASK_ANSWER, ASK_ID, askMessage, askSnapshot } from './ask-fixtures';

const event = (data: AskEvent['data'], seq = 6): AskEvent => ({ conversation_id: ASK_ID, seq, data });
const attachment = { type: 'file', blob: null, name: 'review.txt', path: '.orch/attachments/review.txt', size: 11 } satisfies ContentPart;

describe('Ask stream application', () => {
  it('uses Unicode scalar offsets and preserves non-text parts and message identities', () => {
    const first = askSnapshot({ messages: [askMessage({ content: [{ type: 'text', text: '你' }, attachment, { type: 'text', text: '😀' }], complete: false })] });
    const second = applyAskEvent(first, event({ kind: 'delta', message_id: ASK_ANSWER, offset: 2, text: '好' }));
    expect(second?.messages[0]?.content).toEqual([{ type: 'text', text: '你' }, attachment, { type: 'text', text: '😀好' }]);
    expect(first.messages[0]?.content).toEqual([{ type: 'text', text: '你' }, attachment, { type: 'text', text: '😀' }]);
    expect(second?.messages[0]?.id).toBe(ASK_ANSWER);
    expect(second?.messages[0]?.content[1]).toBe(attachment);
  });
  it('appends the first text delta without replacing an attachment-only message', () => {
    const first = askSnapshot({ messages: [askMessage({ content: [attachment], complete: false })] });
    const second = applyAskEvent(first, event({ kind: 'delta', message_id: ASK_ANSWER, offset: 0, text: 'See the attached notes.' }));
    expect(second?.messages[0]?.content).toEqual([attachment, { type: 'text', text: 'See the attached notes.' }]);
    expect(first.messages[0]?.content).toEqual([attachment]);
    expect(second?.messages[0]?.id).toBe(ASK_ANSWER);
  });
  it('asks for a snapshot on a missing base, wrong offset, reset or deletion', () => {
    const snapshot = askSnapshot();
    expect(applyAskEvent(snapshot, event({ kind: 'delta', message_id: 'missing', offset: 0, text: 'x' }))).toBeNull();
    expect(applyAskEvent(snapshot, event({ kind: 'delta', message_id: ASK_ANSWER, offset: 0, text: 'x' }))).toBeNull();
    expect(applyAskEvent(snapshot, event({ kind: 'reset' }))).toBeNull();
    expect(applyAskEvent(snapshot, event({ kind: 'deleted' }))).toBeNull();
  });
  it('ignores duplicates and another conversation without reintroducing their contents', () => {
    const snapshot = askSnapshot(), update = event({ kind: 'message', message: askMessage({ content: [{ type: 'text', text: 'Replacement' }] }) });
    expect(applyAskEvent(snapshot, { ...update, seq: 5 })).toBe(snapshot);
    expect(applyAskEvent(snapshot, { ...update, conversation_id: 'other' })).toBe(snapshot);
    const next = applyAskEvent(snapshot, update);
    expect(next?.messages).toHaveLength(snapshot.messages.length);
    expect(next?.messages.at(-1)?.content).toEqual([{ type: 'text', text: 'Replacement' }]);
  });
  it.each(['notice', 'error', 'deleted'] as const)('rejects another conversation before any %s side effect', (kind) => {
    const update: AskEvent = { ...event(kind === 'deleted' ? { kind } : { kind, code: 'fixture', message: 'Another organization' }), conversation_id: 'other' };
    expect(acceptsAskEvent(askSnapshot(), update)).toBe(false);
  });
  it.each([NaN, Infinity, 5.5, Number.MAX_SAFE_INTEGER + 1])('rejects an imprecise event sequence %s', (seq) => {
    expect(acceptsAskEvent(askSnapshot(), event({ kind: 'deleted' }, seq))).toBe(false);
  });
  it('carries current permissions from a header replacement', () => {
    const snapshot = askSnapshot();
    const next = applyAskEvent(snapshot, event({ kind: 'conversation', conversation: { ...snapshot.conversation, inspecting: true, can: { message: false, stop: false, keep: false, delete: false, export: true } } }));
    expect(next?.conversation.can.message).toBe(false);
    expect(next?.conversation.inspecting).toBe(true);
    expect(next?.conversation.seq).toBe(6);
  });
  it('keeps newer streamed content when an older snapshot finishes late', () => {
    const old = askSnapshot(), current = applyAskEvent(old, event({ kind: 'message', message: askMessage({ content: [{ type: 'text', text: 'New answer' }] }) }));
    expect(current).not.toBeNull();
    expect(newerAskSnapshot(current ?? undefined, old)).toBe(current);
    expect(newerAskSnapshot(undefined, old)).toBe(old);
  });
  it('reads split UTF-8 and CRLF frames and ignores heartbeat comments', async () => {
    const update = event({ kind: 'delta', message_id: ASK_ANSWER, offset: 0, text: '你😀好' });
    const encoded = new TextEncoder().encode(`: heartbeat\r\n\r\nevent: ask\r\nid: 6\r\ndata: ${JSON.stringify(update)}\r\n\r\n`);
    const received: AskEvent[] = [];
    await readAskEvents(new ReadableStream({ start(controller) { for (const byte of encoded) controller.enqueue(Uint8Array.of(byte)); controller.close(); } }), (event) => received.push(event), new AbortController().signal);
    expect(received).toEqual([update]);
  });
  it('aborts a pending read and releases its stream', async () => {
    let canceled = false;
    const abort = new AbortController();
    const stream = new ReadableStream<Uint8Array>({ cancel() { canceled = true; } });
    const reading = readAskEvents(stream, () => { throw new Error('No event after abort'); }, abort.signal);
    abort.abort(); await reading;
    expect(canceled).toBe(true);
    expect(stream.locked).toBe(false);
  });
});

it.each([[400, 700], [1440, 900]])('keeps a restored floating view on a %ipx by %ipx viewport', (width, height) => {
  const fitted = fitViewWindow({ x: 1800, y: -30, width: 900, height: 1200 }, width, height);
  expect(fitted.x).toBeGreaterThanOrEqual(8); expect(fitted.y).toBeGreaterThanOrEqual(8);
  expect(fitted.x + fitted.width).toBeLessThanOrEqual(width - 8);
  expect(fitted.y + fitted.height).toBeLessThanOrEqual(height - 8);
});

it('respects measured control height while clamping an oversized window to its viewport', () => {
  expect(fitViewWindow({ x: 8, y: 8, width: 420, height: 100 }, 1440, 900, 360).height).toBe(360);
  expect(fitViewWindow({ x: 8, y: 8, width: 420, height: 100 }, 400, 350, 360).height).toBe(334);
});
