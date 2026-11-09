/** Ask uses the ordinary authenticated client and keeps credentials out of event addresses. */
import { infiniteQueryOptions, queryOptions } from '@tanstack/react-query';
import { api, client, ok, repeatable } from '@/api/client';
import type { AskCreateBody, AskEvent, AskGetReply, AskKeepBody, AskKeepReply, AskListReply, AskMessageBody, AskModelsReply, AskReceipt } from '@/generated/api-types';
import { NetworkError, newActKey } from '@/lib/requests';
import { queryClient } from '@/api/query';
import { newerAskSnapshot } from '@/features/ask/events';

/** A request after a wait still belongs to the same admitted browser reader. */
export function askReader(): () => void {
  const reader = client.session.getState();
  return () => {
    const now = client.session.getState();
    if (now.userId !== reader.userId || now.readerVersion !== reader.readerVersion || now.status !== reader.status) throw new DOMException('The conversation reader changed', 'AbortError');
  };
}
export const askKey = (id: string, inspect = false) => ['ask', client.session.getState().userId, client.session.getState().readerVersion, id, inspect] as const;
export const askModelsQuery = (root: string) => {
  const current = askReader();
  return queryOptions({
    queryKey: ['ask-models', client.session.getState().userId, client.session.getState().readerVersion, root] as const,
    queryFn: async ({ signal }): Promise<AskModelsReply> => {
      current(); const reply = await ok(api.GET('/ask/models', { params: { query: { root_org_id: root } }, signal })); current();
      if (reply.root_org_id !== root) throw new NetworkError('malformed'); return reply;
    },
    staleTime: 0,
  });
};
export const askQuery = (id: string, inspect = false) => {
  const key = askKey(id, inspect), current = askReader();
  return queryOptions({
    queryKey: key,
    meta: { requiresAdminMode: inspect },
    queryFn: async ({ signal }): Promise<AskGetReply> => {
      current();
      const reply = await ok(api.GET('/ask/conversations/{id}', { params: { path: { id }, query: { before: null, limit: null } }, signal }));
      current();
      if (reply.conversation.id !== id) throw new NetworkError('malformed');
      return newerAskSnapshot(queryClient.getQueryData<AskGetReply>(key), reply);
    },
    staleTime: 0,
  });
};

export const askHistoryQuery = (root?: string, inspect = false) => {
  const current = askReader();
  return infiniteQueryOptions({
    queryKey: ['ask-history', client.session.getState().userId, client.session.getState().readerVersion, root ?? 'all', inspect] as const,
    meta: { requiresAdminMode: inspect },
    initialPageParam: null as string | null,
    queryFn: async ({ pageParam, signal }): Promise<AskListReply> => {
      current();
      const reply = await ok(api.GET('/ask/conversations', { params: { query: {
        root_org_id: root ?? null, cursor: pageParam, limit: null, inspect,
      } }, signal }));
      current(); return reply;
    },
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
};

export function createAsk(body: AskCreateBody, key = newActKey()): Promise<AskReceipt> {
  return ok(api.POST('/ask/conversations', { body, ...repeatable(key) }));
}
/** Preserving a saved choice omits model; a supplied null explicitly resets it. */
export function sendAsk(id: string, body: AskMessageBody, keepModel = false): Promise<AskReceipt> {
  return ok(api.POST('/ask/conversations/{id}/messages', { params: { path: { id } }, body,
    bodySerializer: () => JSON.stringify(keepModel ? Object.fromEntries(Object.entries(body).filter(([field]) => field !== 'model')) : body),
  }));
}
export function stopAsk(id: string): Promise<AskReceipt> {
  return ok(api.POST('/ask/conversations/{id}/stop', { params: { path: { id } } }));
}
export function keepAsk(id: string, body: AskKeepBody, options: { key?: string; toTurn?: number } = {}): Promise<AskKeepReply> {
  const request = { ...body, ...(options.toTurn === undefined ? {} : { to_turn: options.toTurn }) };
  return ok(api.POST('/ask/conversations/{id}/keep', { params: { path: { id } }, body: request, ...repeatable(options.key ?? newActKey()) }));
}
export function exportAsk(id: string): Promise<Blob> { return ok(api.GET('/ask/conversations/{id}/export', { params: { path: { id } }, parseAs: 'blob' })); }
export function olderAsk(id: string, before: string, signal?: AbortSignal): Promise<AskGetReply> {
  return ok(api.GET('/ask/conversations/{id}', { params: { path: { id }, query: { before, limit: null } }, ...(signal === undefined ? {} : { signal }) }));
}

/** TextDecoder preserves split UTF-8 characters; SSE frames may cross any transport chunk. */
export async function readAskEvents(stream: ReadableStream<Uint8Array>, receive: (event: AskEvent) => void, signal: AbortSignal): Promise<void> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let pending = '';
  const cancel = () => { void reader.cancel().catch(() => undefined); };
  signal.addEventListener('abort', cancel, { once: true });
  try {
    while (!signal.aborted) {
      const { value, done } = await reader.read();
      pending += decoder.decode(value, { stream: !done });
      // Normalize only complete line endings: a CRLF may itself cross a chunk.
      pending = pending.replace(/\r\n/g, '\n');
      let boundary: number;
      while ((boundary = pending.indexOf('\n\n')) >= 0) {
        const frame = pending.slice(0, boundary); pending = pending.slice(boundary + 2);
        const lines = frame.split('\n');
        const name = lines.find((line) => line.startsWith('event:'))?.slice(6).trim();
        const data = lines.filter((line) => line.startsWith('data:')).map((line) => line.slice(5).replace(/^ /, '')).join('\n');
        if (name === 'ask' && data !== '' && !signal.aborted) receive(JSON.parse(data) as AskEvent);
      }
      if (done) return;
    }
  } finally { signal.removeEventListener('abort', cancel); await reader.cancel().catch(() => undefined); reader.releaseLock(); }
}

export async function streamAsk(id: string, after: number, receive: (event: AskEvent) => void, signal: AbortSignal): Promise<void> {
  const stream = await ok(api.GET('/ask/conversations/{id}/events', { params: { path: { id }, query: { after } }, parseAs: 'stream', signal }));
  if (stream !== null) await readAskEvents(stream, receive, signal);
}
