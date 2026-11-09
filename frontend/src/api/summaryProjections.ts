/** Search observers share one native stored-read batch per reader/turn, never a paid summary act. */
import { queryOptions } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { summaryReaderKey } from '@/api/summaryReader';
import type { SummaryProjection, SummaryTarget } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';

interface Waiter { target: SummaryTarget; signal: AbortSignal; resolve: (value: SummaryProjection) => void; reject: (error: unknown) => void; abort: () => void; cancelled: boolean; done: boolean; group?: Group }
interface Pending { reader: string; waiters: Waiter[] }
interface Group { controller: AbortController; waiters: Waiter[] }
const pending = new Map<string, Pending>();
const keyOf = (target: SummaryTarget) => `${target.kind}:${target.id}`;
const retired = () => new DOMException('The summary opening changed', 'AbortError');

function finish(waiter: Waiter, value: SummaryProjection | undefined, error?: unknown): void {
  if (waiter.done) return;
  waiter.done = true; waiter.signal.removeEventListener('abort', waiter.abort);
  if (error !== undefined) waiter.reject(error); else if (value !== undefined) waiter.resolve(value);
}

async function flush(queue: Pending): Promise<void> {
  if (pending.get(queue.reader) === queue) pending.delete(queue.reader);
  const waiters = queue.waiters.filter((waiter) => !waiter.cancelled);
  if (waiters.length === 0) return;
  const group: Group = { controller: new AbortController(), waiters };
  for (const waiter of waiters) waiter.group = group;
  const targets = [...new Map(waiters.map((waiter) => [keyOf(waiter.target), waiter.target])).values()];
  const byTarget = new Map<string, Waiter[]>();
  for (const waiter of waiters) { const key = keyOf(waiter.target); byTarget.set(key, [...(byTarget.get(key) ?? []), waiter]); }
  let offset = 0;
  try {
    do {
      if (queue.reader !== summaryReaderKey()) throw retired();
      const reply = await ok(api.POST('/summaries/batch', { body: { targets, offset }, signal: group.controller.signal }));
      if (queue.reader !== summaryReaderKey()) throw retired();
      for (const item of reply.items) {
        const matches = byTarget.get(keyOf(item.target));
        if (matches === undefined || item.root_org_id === '') throw new NetworkError('malformed');
        for (const waiter of matches) finish(waiter, item);
      }
      for (const target of reply.unavailable) {
        const matches = byTarget.get(keyOf(target));
        if (matches === undefined) throw new NetworkError('malformed');
        for (const waiter of matches) finish(waiter, undefined, new NetworkError('failed'));
      }
      if (reply.next_offset === null) break;
      if (!Number.isSafeInteger(reply.next_offset) || reply.next_offset <= offset || reply.next_offset >= targets.length) throw new NetworkError('malformed');
      offset = reply.next_offset;
    } while (!group.controller.signal.aborted);
    for (const waiter of waiters) finish(waiter, undefined, new NetworkError('malformed'));
  } catch (error) {
    for (const waiter of waiters) finish(waiter, undefined, error);
  }
}

function read(target: SummaryTarget, reader: string, signal: AbortSignal): Promise<SummaryProjection> {
  return new Promise((resolve, reject) => {
    if (reader !== summaryReaderKey() || signal.aborted) { reject(retired()); return; }
    let queue = pending.get(reader);
    if (queue === undefined) {
      queue = { reader, waiters: [] }; pending.set(reader, queue);
      const selected = queue;
      // End-of-turn IO coalescing joins simultaneous observers; it is not a debounce/content limit.
      setTimeout(() => { void flush(selected); }, 0);
    }
    const waiter: Waiter = { target, signal, resolve, reject, cancelled: false, done: false, abort: () => undefined };
    waiter.abort = () => {
      waiter.cancelled = true; finish(waiter, undefined, retired());
      if (waiter.group?.waiters.every((other) => other.cancelled || other.done)) waiter.group.controller.abort();
    };
    signal.addEventListener('abort', waiter.abort, { once: true });
    queue.waiters.push(waiter);
  });
}

export function searchSummaryProjectionQuery(target: SummaryTarget) {
  const reader = summaryReaderKey();
  return queryOptions({ queryKey: ['search-summary-projection', reader, target.kind, target.id] as const,
    queryFn: ({ signal }) => read(target, reader, signal), staleTime: 0, retry: false });
}
