/** Apply ordered snapshots and Unicode-scalar deltas; an unavailable base always needs a fresh read. */
import type { AskEvent, AskGetReply } from '@/generated/api-types';

/** A slow snapshot cannot replace events already received after its own cut. */
export function newerAskSnapshot(held: AskGetReply | undefined, reply: AskGetReply): AskGetReply {
  return held?.conversation.id === reply.conversation.id && held.conversation.seq > reply.conversation.seq ? held : reply;
}

/** Every event effect, including notices and deletion, belongs to this opening and a precise sequence. */
export function acceptsAskEvent(snapshot: AskGetReply, event: AskEvent): boolean {
  return event.conversation_id === snapshot.conversation.id && Number.isSafeInteger(event.seq) && event.seq > snapshot.conversation.seq;
}

export function applyAskEvent(snapshot: AskGetReply, event: AskEvent): AskGetReply | null {
  if (!acceptsAskEvent(snapshot, event)) return snapshot;
  const data = event.data;
  if (data.kind === 'reset' || data.kind === 'deleted') return null;
  let next = { ...snapshot, conversation: { ...snapshot.conversation, seq: event.seq } };
  if (data.kind === 'conversation') return { ...next, conversation: { ...data.conversation, seq: event.seq } };
  if (data.kind === 'message') {
    const previous = snapshot.messages.findIndex((message) => message.id === data.message.id);
    return { ...next, messages: previous < 0 ? [...snapshot.messages, data.message] : snapshot.messages.map((message, index) => index === previous ? data.message : message) };
  }
  if (data.kind === 'delta') {
    const message = snapshot.messages.find((message) => message.id === data.message_id);
    if (message === undefined) return null;
    const text = message.content.flatMap((part) => part.type === 'text' ? [part.text] : []).join('');
    if (Array.from(text).length !== data.offset) return null;
    const last = message.content.findLastIndex((part) => part.type === 'text');
    const content = last < 0 ? [...message.content, { type: 'text' as const, text: data.text }] : message.content.map((part, index) => index === last && part.type === 'text' ? { ...part, text: part.text + data.text } : part);
    next = { ...next, messages: snapshot.messages.map((one) => one.id === message.id ? { ...one, content } : one) };
  }
  return next;
}
