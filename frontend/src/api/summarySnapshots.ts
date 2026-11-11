/** Full native summary views share the visible opening's read, without sharing authority or ranges. */
import type { QueryClient } from '@tanstack/react-query';
import { api, ok } from '@/api/client';
import { summaryReaderKey, resourceSummaryKey } from '@/api/summaryReader';
import type { DocumentSync, NodeChatOpening, SummaryChatRange, SummaryProjection, SummarySnapshot, SummaryTarget } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';

interface Waiter {
  target: SummaryTarget; range: SummaryChatRange | undefined; signal: AbortSignal;
  resolve: (value: SummarySnapshot) => void; reject: (error: unknown) => void;
  abort: () => void; done: boolean; group?: Group;
}
interface Group { waiters: Waiter[]; controller: AbortController }
interface Pending { reader: string; client: QueryClient; waiters: Waiter[] }
const pending = new WeakMap<QueryClient, Map<string, Pending>>();
const targetKey = (target: SummaryTarget) => `${target.kind}:${target.id}`;
const rangeKey = (range: SummaryChatRange | undefined) => JSON.stringify([range?.after ?? null, range?.through ?? null]);
const retired = () => new DOMException('The summary reader changed', 'AbortError');
const nextPaint = () => new Promise<void>((resolve) => {
  if (typeof requestAnimationFrame === 'function' && (typeof document === 'undefined' || document.visibilityState !== 'hidden')) requestAnimationFrame(() => resolve());
  else resolve();
});
function finish(waiter: Waiter, value: SummarySnapshot | undefined, error?: unknown) {
  if (waiter.done) return;
  waiter.done = true; waiter.signal.removeEventListener('abort', waiter.abort);
  if (error !== undefined) waiter.reject(error); else if (value !== undefined) waiter.resolve(value);
}

async function readGroup(queue: Pending, waiters: Waiter[]): Promise<void> {
  const group: Group = { waiters, controller: new AbortController() };
  for (const waiter of waiters) waiter.group = group;
  const targets = [...new Map(waiters.map((waiter) => [targetKey(waiter.target), waiter.target])).values()];
  const ranges: Record<string, SummaryChatRange> = Object.fromEntries(waiters.flatMap((waiter) =>
    waiter.target.kind === 'chat' && waiter.range !== undefined ? [[waiter.target.id, waiter.range] as const] : []));
  let offset = 0;
  try {
    do {
      if (queue.reader !== summaryReaderKey()) throw retired();
      const body = { targets, offset, full: true, ranges };
      const reply = await ok(api.POST('/summaries/batch', { body, signal: group.controller.signal }));
      if (queue.reader !== summaryReaderKey()) throw retired();
      for (const projection of reply.items) {
        // The optional member is declared by the native full-batch contract; generated base fields remain shared.
        const item: SummaryProjection & { full?: SummarySnapshot | null } = projection;
        const matches = waiters.filter((waiter) => targetKey(waiter.target) === targetKey(item.target));
        const view = item.full;
        if (matches.length === 0 || view === undefined || view === null || view.root_org_id === ''
          || view.root_org_id !== item.root_org_id || targetKey(view.target) !== targetKey(item.target)) throw new NetworkError('malformed');
        for (const waiter of matches) finish(waiter, view);
      }
      for (const target of reply.unavailable) {
        const matches = waiters.filter((waiter) => targetKey(waiter.target) === targetKey(target));
        if (matches.length === 0) throw new NetworkError('malformed');
        for (const waiter of matches) finish(waiter, undefined, new NetworkError('failed'));
      }
      if (reply.next_offset === null) break;
      if (!Number.isSafeInteger(reply.next_offset) || reply.next_offset <= offset || reply.next_offset >= targets.length) throw new NetworkError('malformed');
      offset = reply.next_offset;
    } while (!group.controller.signal.aborted);
    for (const waiter of waiters) finish(waiter, undefined, new NetworkError('malformed'));
  } catch (error) { for (const waiter of waiters) finish(waiter, undefined, error); }
}

async function flush(queue: Pending): Promise<void> {
  // A paint collects mounted observers. The actual in-flight chat opening then supplies its
  // catch-up/title observers; no fixed network delay or content cap decides the cohort.
  await nextPaint();
  // The second frame is after the first paint's passive effects have mounted the chat opening.
  await nextPaint();
  const nodes = new Set(queue.waiters.filter((waiter) => waiter.target.kind === 'thread' || waiter.target.kind === 'topic').map((waiter) => waiter.target.id));
  const openings = queue.client.getQueryCache().findAll().filter((query) => query.queryKey[0] === 'node-chat-opening'
    && typeof query.queryKey[1] === 'string' && nodes.has(query.queryKey[1]) && query.state.fetchStatus === 'fetching');
  await Promise.allSettled(openings.map((query) => query.promise).filter((promise) => promise !== undefined));
  if (openings.length > 0) await nextPaint();
  pending.get(queue.client)?.delete(queue.reader);
  const groups: Waiter[][] = [];
  for (const waiter of queue.waiters.filter((waiter) => !waiter.done)) {
    if (queue.reader !== summaryReaderKey()) { finish(waiter, undefined, retired()); continue; }
    const carried = queue.client.getQueryData<SummarySnapshot>(resourceSummaryKey(waiter.target, waiter.range));
    if (carried !== undefined && openingData.has(carried)) { claimedOpeningData.add(carried); finish(waiter, carried); continue; }
    // Two openings of the same chat with different captured ranges never alias one another.
    let group = groups.find((group) => group.every((other) => targetKey(other.target) !== targetKey(waiter.target) || rangeKey(other.range) === rangeKey(waiter.range)));
    if (group === undefined) { group = []; groups.push(group); }
    group.push(waiter);
  }
  for (const group of groups) { const active = group.filter((waiter) => !waiter.done); if (active.length > 0) await readGroup(queue, active); }
}

export function readSummarySnapshot(client: QueryClient, target: SummaryTarget, range: SummaryChatRange | undefined, reader: string, signal: AbortSignal, cold = true): Promise<SummarySnapshot> {
  return new Promise((resolve, reject) => {
    if (reader !== summaryReaderKey() || signal.aborted) { reject(retired()); return; }
    const waiter: Waiter = { target, range, signal, resolve, reject, abort: () => undefined, done: false };
    waiter.abort = () => {
      finish(waiter, undefined, retired());
      if (waiter.group?.waiters.every((other) => other.done)) waiter.group.controller.abort();
    };
    signal.addEventListener('abort', waiter.abort, { once: true });
    // Polling follows the server's clock directly; only cold openings need a mounting cohort.
    if (!cold) { void readGroup({ reader, client, waiters: [waiter] }, [waiter]); return; }
    let queues = pending.get(client);
    if (queues === undefined) { queues = new Map(); pending.set(client, queues); }
    let queue = queues.get(reader);
    if (queue === undefined) {
      queue = { reader, client, waiters: [] }; queues.set(reader, queue);
      const selected = queue;
      setTimeout(() => { void flush(selected); }, 0);
    }
    queue.waiters.push(waiter);
  });
}


// These one-use mounting markers save only the just-completed opening's duplicate read.
// They are not stale-time extensions or authorization: a later sole mount reads again.
const openingData = new WeakSet<SummarySnapshot>();
const claimedOpeningData = new WeakSet<SummarySnapshot>();
export function refetchSummaryOpening(data: SummarySnapshot | undefined, observers: number, invalidated = false): boolean {
  if (invalidated || data === undefined || !openingData.has(data)) return true;
  if (!claimedOpeningData.has(data)) { if (observers > 0) claimedOpeningData.add(data); return false; }
  return observers <= 1;
}
export function adoptSummaryOpening(cache: QueryClient, value: NodeChatOpening, nodeId: string, selectedChatCurrent = true): void {
  const opening: NodeChatOpening & { summaries?: SummarySnapshot[]; summary_range?: SummaryChatRange | null } = value;
  const chat = opening.opening?.sync.chat;
  for (const snapshot of opening.summaries ?? []) {
    const ownTopic = snapshot.target.kind === 'topic' && snapshot.target.id === nodeId;
    const selectedChat = chat !== undefined && (snapshot.target.kind === 'chat' || snapshot.target.kind === 'chat_title') && snapshot.target.id === chat.id;
    if ((!ownTopic && !selectedChat) || snapshot.root_org_id === '' || (chat !== undefined && snapshot.root_org_id !== chat.org_id)) throw new NetworkError('malformed');
    if (selectedChat && !selectedChatCurrent) continue;
    const range = snapshot.target.kind === 'chat' ? opening.summary_range ?? undefined : undefined;
    seedSummaryOpening(cache, snapshot, range);
  }
}

/** A full live document sync can supply the opening summary for its own reader and document. */
export function adoptDocumentSummaryOpening(cache: QueryClient, value: DocumentSync, id: string, reader: string): void {
  if (reader !== summaryReaderKey() || !value.complete || value.since !== null || value.document.deleted || value.document.id !== id) return;
  const opening: DocumentSync & { summary?: SummarySnapshot | null } = value;
  const summary = opening.summary;
  const root = value.document.path[0];
  if (summary?.target.kind !== 'document' || summary.target.id !== id
    || summary.root_org_id === '' || (root?.kind === 'org' && summary.root_org_id !== root.id)) return;
  seedSummaryOpening(cache, summary);
}

/** Called only with the checked current opening's snapshot; later mounts remain ordinary reads. */
export function seedSummaryOpening(cache: QueryClient, snapshot: SummarySnapshot, range?: SummaryChatRange): void {
  const stored = cache.setQueryData<SummarySnapshot>(resourceSummaryKey(snapshot.target, range), snapshot);
  if (stored !== undefined) { openingData.add(stored); claimedOpeningData.delete(stored); }
}

export function forgetSummaryOpening(data: SummarySnapshot | undefined): void {
  if (data !== undefined) { openingData.delete(data); claimedOpeningData.delete(data); }
}
