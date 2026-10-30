/** History-scoped thread feed without the application's global socket or visual hooks. */
import { isCompleteThread } from '@/api/cache';
import { client } from '@/api/client';
import { fetchThreadSync } from '@/api/threads';
import { captureThreadRead, isCurrentThreadRead, stampThreadRows, threadReadSelector } from '@/api/threadRead';
import { type DurableFeed, type FeedRegistry, loadWithRetries } from '@/realtime/sync';
import { setThreadReadChangeHandler, setThreadReadHolder, setThreadReadRetainer,
  threadViewOf, threadWriter, type ThreadRows, type ThreadView } from '@/realtime/stores/thread';
import { matchesThreadProjection, projectionFitsSelection, readThreadProjection, sameThreadProjection, threadReceipt, type ThreadReadCapture, type ThreadReadRef, type ThreadReadTarget, sameThreadRead, threadReadRegistryKey, threadReadTarget } from '@/realtime/threadScope';

export type ThreadFeedData = ThreadRows & { hasOlder?: boolean };

/** Apply only admitted complete row views. Root's shell callback may then update global metadata. */
export function applyThreadFeedRows(threadId: ThreadReadTarget, data: ThreadFeedData, reset: boolean): ThreadView | null {
  const captured = threadReceipt(data);
  if (captured !== undefined && (!sameThreadRead(captured.ref,threadId) || !isCurrentThreadRead(captured))) return null;
  if (data.thread !== undefined && data.thread !== null && (!isCompleteThread(data.thread) || data.thread.id !== threadReadTarget(threadId).threadId)) {
    const error = new Error('The live reply did not carry this complete thread view');
    if (captured === undefined) threadWriter.failed(threadId,error);
    else threadWriter.refused(threadId,error,captured);
    return null;
  }
  const before = threadViewOf(threadId);
  if (reset) threadWriter.reset(threadId, data, data.hasOlder === true, captured);
  else threadWriter.apply(threadId, data, captured);
  return before;
}

interface ThreadReadHooks {
  rows: (data: ThreadFeedData, reset: boolean) => void;
  subscribed?: () => void;
}

export function createThreadReadFeed(threadId: ThreadReadTarget, hooks: ThreadReadHooks): DurableFeed<ThreadFeedData> {
  let delivered: ThreadReadCapture | null = null;
  let deliveredSeq: number | null = null;
  let rejected: ThreadReadCapture | null = null;
  return {
    feed: `thread:${threadReadTarget(threadId).threadId}`,
    scope: () => {
      const captured = captureThreadRead(threadId);
      const selector = threadReadSelector(captured);
      return {
        ...(selector.continuation_id === undefined || selector.continuation_id === null ? {} : { continuationId: selector.continuation_id }),
        ...(selector.projection_revision === undefined || selector.projection_revision === null ? {} : { projectionRevision: selector.projection_revision }),
        accept: (frame) => {
          if (!isCurrentThreadRead(captured)) return frame.op === 'subscribed' ? 'reload' : 'ignore';
          const held = threadViewOf(threadId).read;
          if (held === null || !matchesThreadProjection(held, captured)) return 'ignore';
          if (frame.op === 'resync' && (held.projection !== null || held.switched)) {
            threadWriter.invalidate(threadId, false);
            return 'reload';
          }
          const data = frame.op === 'sync' ? frame.data : null;
          const value = frame.op === 'subscribed' ? frame.projection : data?.projection;
          const projection = readThreadProjection(value);
          const scoped = frame.op === 'sync' || frame.op === 'subscribed';
          if (data?.thread !== undefined && data.thread !== null && (!isCompleteThread(data.thread) || data.thread.id !== threadReadTarget(threadId).threadId)
            || value !== undefined && value !== null && projection === null
            || scoped && projection !== null && !projectionFitsSelection(held,projection)
            || scoped && projection === null && (held.projection !== null || held.switched || held.selection !== null)) {
            rejected = captured;
            return 'refuse';
          }
          if (data?.complete === false || scoped && projection !== null
            && (held.projection === null || !sameThreadProjection(held.projection, projection))) {
            threadWriter.invalidate(threadId, false);
            return 'reload';
          }
          delivered = captured;
          deliveredSeq = frame.op === 'sync' ? frame.seq : null;
          return 'accept';
        },
      };
    },
    resetting: () => { threadWriter.beginRead(threadId, client.session.getState().readerVersion); },
    load: async (since, signal) => {
      const reply = await loadWithRetries(() => fetchThreadSync(threadId, since, signal), signal);
      const captured = threadReceipt(reply) ?? captureThreadRead(threadId);
      const data = stampThreadRows({ ...reply, hasOlder: reply.has_older }, captured);
      return { seq: reply.seq, complete: reply.complete, data };
    },
    reset: (data) => hooks.rows(data, true),
    apply: (data) => hooks.rows(data, false),
    read: (data) => {
      const rows: ThreadFeedData = { ...data, ...(deliveredSeq === null ? {} : { seq: deliveredSeq }) };
      return delivered === null ? rows : stampThreadRows(rows, delivered);
    },
    failed: (error) => threadWriter.failed(threadId, error),
    refused: (error) => { if (rejected !== null) threadWriter.refused(threadId, error, rejected); },
    closed: (reason) => threadWriter.closed(threadId, reason),
    subscribed: () => hooks.subscribed?.(),
    other: (frame) => {
      if (delivered === null || !isCurrentThreadRead(delivered)) return;
      if (frame.op === 'delta') {
        threadWriter.delta(threadId, {
          item_id: frame.item_id, field: frame.field, offset: frame.offset, len: frame.len, data: frame.data,
        }, delivered);
      } else if (frame.op === 'presence') threadWriter.presence(threadId, frame.users);
    },
  };
}


export interface ThreadReadCoordinator {
  (target: ThreadReadTarget): void;
  rebase: (target: ThreadReadTarget) => void;
}

/** The same cache owns streaming and explicitly demanded HTTP-only history recovery. */
export function coordinateThreadReads(registry: FeedRegistry, hooks: (threadId: string, ref: ThreadReadRef) => ThreadReadHooks): ThreadReadCoordinator {
  const key = (threadId: ThreadReadTarget) => threadReadRegistryKey(threadId);
  const reload = (threadId: ThreadReadTarget): void => {
    const ref = threadReadTarget(threadId);
    const feed = createThreadReadFeed(ref,hooks(ref.threadId,ref));
    let captured: ThreadReadCapture | null = null;
    registry.reload(key(threadId), {
      load: async (signal) => {
        const pending = feed.load(null,signal);
        captured = captureThreadRead(threadId);
        const loaded = await pending;
        captured = threadReceipt(loaded.data) ?? captureThreadRead(threadId);
        if (!signal.aborted) feed.reset(loaded.data,loaded.seq);
      },
      failed: (error) => { if (captured !== null && isCurrentThreadRead(captured)) feed.failed?.(error); },
    });
  };
  const rebase = (target: ThreadReadTarget): void => {
    const ref = threadReadTarget(target);
    threadWriter.rebase(ref,client.session.getState().readerVersion);
    const before = threadViewOf(ref).read;
    // The registry finishes stopping/reloading its follower first. HTTP-only visible demand
    // then gets one opening; idle entries and already-opening streams do no extra work.
    queueMicrotask(() => {
      const view = threadViewOf(ref), current = view.read;
      if (before !== null && current?.identity === before.identity && current.epoch === before.epoch
        && !current.provisional && !current.ready && !current.opening && view.error === null && view.closed === null) reload(ref);
    });
  };
  setThreadReadChangeHandler((_id,ref) => reload(ref));
  setThreadReadRetainer((_id,ref) => registry.retain(key(ref),() => threadWriter.evict(ref),() => rebase(ref)));
  setThreadReadHolder((_id,ref) => {
    const release = registry.hold(key(ref),() => threadWriter.evict(ref),() => rebase(ref));
    const view = threadViewOf(ref);
    const scope = view.read;
    // A returning visible HTTP owner may finish a selected read that had no owner at selection.
    // Failed/refused/closed reads require explicit Retry; this never creates an idle repair loop.
    if (scope !== null && scope.reader === client.session.getState().readerVersion
      && !scope.provisional && !scope.ready && !scope.opening && view.error === null && view.closed === null) reload(ref);
    return release;
  });
  return Object.assign(reload,{ rebase });
}

/** True tab close is distinct from a hidden owner releasing its normal retained lease. */
export function forgetThreadRead(registry: FeedRegistry, target: ThreadReadTarget): void {
  if (!registry.forget(threadReadRegistryKey(target))) threadWriter.evict(target);
}
