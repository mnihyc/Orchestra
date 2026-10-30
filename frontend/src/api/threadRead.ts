/** One captured reader/history identity for positioned HTTP and returned rows. */
import { ApiError, client } from '@/api/client';
import { queryClient } from '@/api/query';
import type { ThreadContinuationProjection } from '@/generated/api-types';
import { holdThreadRead, threadViewOf, threadWriter, useThreadStore, type ThreadRows } from '@/realtime/stores/thread';
import { matchesThreadCapture, matchesThreadProjection, projectionFitsSelection, sameThreadProjection,
  coldThreadKey, scopedThreadKey, threadReadTarget, stampPromptReceipt, stampThreadReceipt, threadReadChanged, threadReceipt, threadReadSelector,
  type ThreadReadCapture, type ThreadReadTarget } from '@/realtime/threadScope';

export { threadReadSelector };
export type { ThreadReadCapture, ThreadReadTarget };

export function captureThreadRead(threadId: ThreadReadTarget): ThreadReadCapture {
  return threadWriter.capture(threadId, client.session.getState().readerVersion);
}

export function isCurrentThreadRead(captured: ThreadReadCapture, signal?: AbortSignal): boolean {
  const scope = threadViewOf(captured.ref).read;
  return signal?.aborted !== true && !captured.scope.controller.signal.aborted && captured.scope.reader === client.session.getState().readerVersion
    && matchesThreadCapture(scope, captured) && scope !== null && matchesThreadProjection(scope, captured);
}

export function assertThreadRead(captured: ThreadReadCapture, signal?: AbortSignal): void {
  const scope = threadViewOf(captured.ref).read;
  if (!isCurrentThreadRead(captured, signal) || scope === null || !matchesThreadProjection(scope, captured)
    || threadViewOf(captured.ref).closed !== null) throw threadReadChanged();
}

/** Existing execution APIs retain their active behavior; historical selection cannot invoke them. */
export function captureThreadExecution(threadId: ThreadReadTarget): ThreadReadCapture {
  const captured = captureThreadRead(threadId);
  const scope = captured.scope;
  if (scope.selection !== null || !scope.provisional && !scope.ready || threadViewOf(threadId).closed !== null
    || scope.projection !== null && scope.projection.viewed_continuation_id !== scope.projection.active_continuation_id) throw threadReadChanged();
  return captured;
}

export function selectThreadContinuation(threadId: ThreadReadTarget, continuationId: string | null): void {
  threadWriter.select(threadId, continuationId, client.session.getState().readerVersion);
}

export function threadReadQueryKey(threadId: ThreadReadTarget, base: readonly unknown[]): readonly unknown[] {
  const scope = threadViewOf(threadId).read;
  return scope === null ? coldThreadKey(client.session.getState().readerVersion, base, threadId) : scopedThreadKey(scope, base);
}

/** A stale positioned read never merges its body. Only its current demand may request a fresh opening. */
export function threadReadFailure(captured: ThreadReadCapture, error: unknown, signal?: AbortSignal): never {
  if (!isCurrentThreadRead(captured, signal)) throw threadReadChanged();
  if (error instanceof ApiError && error.is('conflict.stale_revision')) threadWriter.invalidate(captured.ref);
  throw error;
}

/** Modern positioned pages must echo the actual projection; legacy omissions are default-only. */
export function receiveThreadRead(captured: ThreadReadCapture, projection: ThreadContinuationProjection | null | undefined,
  signal?: AbortSignal): ThreadReadCapture {
  assertThreadRead(captured, signal);
  const expected = captured.scope.projection;
  if (projection === undefined || projection === null) {
    if (expected !== null || captured.scope.switched || captured.scope.selection !== null) {
      const error = new Error('The reply did not identify the selected history');
      threadWriter.refused(captured.ref, error, captured);
      throw error;
    }
    return captured;
  }
  if (!projectionFitsSelection(captured.scope, projection) || expected !== null && !sameThreadProjection(expected, projection)) {
    threadWriter.invalidate(captured.ref);
    throw threadReadChanged();
  }
  if (expected === null) threadWriter.apply(captured.ref, { projection }, captured);
  return captureThreadRead(captured.ref);
}

export function stampThreadRows<Rows extends ThreadRows>(rows: Rows, captured: ThreadReadCapture): Rows {
  for (const row of [...(rows.items ?? []), ...(rows.turns ?? []), ...(rows.inputs ?? []), ...(rows.segments ?? [])]) stampThreadReceipt(row, captured);
  if (rows.thread !== undefined && rows.thread !== null) stampThreadReceipt(rows.thread, captured);
  const summaries = new Map((rows.prompt_summaries ?? []).map((summary) => [summary.input_id, summary]));
  for (const item of rows.items ?? []) if (item.kind === 'user_message' && item.payload.input_id !== null) {
    const summary = summaries.get(item.payload.input_id);
    if (summary !== undefined) stampPromptReceipt(item, summary);
  }
  return stampThreadReceipt(rows, captured);
}

/** Capturing before constructing the request also fences a query scheduled after its key went stale. */
export async function readInThread<Value>(captured: ThreadReadCapture, read: (signal: AbortSignal) => Promise<Value>,
  signal?: AbortSignal, opening = false): Promise<Value> {
  const requestSignal = signal === undefined ? captured.scope.controller.signal
    : AbortSignal.any([signal, captured.scope.controller.signal]);
  assertThreadRead(captured, requestSignal);
  if (!opening && !captured.scope.provisional && !captured.scope.ready) throw threadReadChanged();
  const release = holdThreadRead(captured.ref);
  try {
    const value = await read(requestSignal);
    assertThreadRead(captured, requestSignal);
    return value;
  } catch (error) { return threadReadFailure(captured, error, signal); }
  finally { release(); }
}

/** Capture before POST/PATCH; a late action receipt cannot populate a different read selection. */
export async function threadAct<Value extends object>(threadId: ThreadReadTarget, act: () => Promise<Value>,
  rows: (value: Value) => readonly object[] = () => []): Promise<Value> {
  const captured = captureThreadExecution(threadId);
  const reply = await readInThread(captured, act);
  for (const row of rows(reply)) stampThreadReceipt(row, captured);
  return stampThreadReceipt(reply, captured);
}

/** Query option factories are pure. Registration/cancellation happens only when their read starts. */
export function threadQueryRead(threadId: ThreadReadTarget, base: readonly unknown[]) {
  const ref = threadReadTarget(threadId);
  const reader = client.session.getState().readerVersion;
  const initial = threadViewOf(ref).read;
  let bound: ThreadReadCapture | null = initial === null ? null : { threadId: ref.threadId, ref, scope: initial };
  const key = initial === null ? coldThreadKey(reader, base, ref) : scopedThreadKey(initial, base);
  const capture = async (signal: AbortSignal, positioned = false): Promise<ThreadReadCapture> => {
    if (signal.aborted || reader !== client.session.getState().readerVersion) throw threadReadChanged();
    let current = threadViewOf(threadId).read;
    if (positioned) {
      const data = queryClient.getQueryData(key);
      const first: unknown = typeof data === 'object' && data !== null && 'pages' in data && Array.isArray(data.pages) ? data.pages[0] : undefined;
      const receipt = typeof first === 'object' && first !== null ? threadReceipt(first) : undefined;
      if (receipt !== undefined && !isCurrentThreadRead(receipt, signal)) throw threadReadChanged();
    }
    if (bound !== null && (current === null || !matchesThreadCapture(current, bound))
      || initial !== null && (current?.identity !== initial.identity || current.epoch !== initial.epoch)
      || initial === null && current !== null && (!current.provisional || current.switched || current.selection !== null)) throw threadReadChanged();
    if (current?.opening === true) {
      const waiting = captureThreadRead(threadId);
      await new Promise<void>((resolve, reject) => {
        let unsubscribe: () => void = () => undefined;
        const finish = (error?: unknown) => {
          unsubscribe(); signal.removeEventListener('abort', abort); waiting.scope.controller.signal.removeEventListener('abort', abort);
          if (error === undefined) resolve(); else reject(error instanceof Error ? error : threadReadChanged());
        };
        const abort = () => finish(threadReadChanged());
        const inspect = () => {
          const view = threadViewOf(threadId);
          if (!isCurrentThreadRead(waiting, signal)) finish(threadReadChanged());
          else if (view.error !== null) finish(view.error);
          else if (view.read?.opening !== true) finish();
        };
        unsubscribe = useThreadStore.subscribe(inspect);
        signal.addEventListener('abort', abort, { once: true });
        waiting.scope.controller.signal.addEventListener('abort', abort, { once: true });
        inspect();
      });
      current = threadViewOf(threadId).read;
    }
    if (current !== null && !current.provisional && !current.ready) throw threadReadChanged();
    const captured = captureThreadRead(threadId);
    assertThreadRead(captured, signal);
    bound = captured;
    return captured;
  };
  return { key, capture, enabled: initial === null || initial.provisional || initial.ready };
}
