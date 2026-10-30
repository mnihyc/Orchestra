/** Version arrows only read adjacent history. Execution changes use explicit replay-bound acts. */
import { ApiError, api, ok } from '@/api/client';
import { captureThreadRead, readInThread, receiveThreadRead, threadReadSelector } from '@/api/threadRead';
import { sameThreadProjection, stampThreadReceipt, threadReadChanged, type ThreadReadTarget } from '@/realtime/threadScope';
import type {
  ThreadContinuationAccepted,
  ThreadContinuationActivateBody,
  ThreadContinuationProjection,
  ThreadPromptSummary,
  ThreadPromptVersionCreateBody,
  ThreadPromptVersionsPage,
} from '@/generated/api-types';

/** A failed commit/response can still have taken effect; explicit retries keep their original identity. */
export function continuationOutcomeUnknown(error: unknown): boolean {
  return error !== null && error !== undefined && (!(error instanceof ApiError) || error.status >= 500);
}

/** The actual source projection accompanies every positioned read; ordinals never identify a branch. */
export function continuationSelector(projection: ThreadContinuationProjection) {
  return { ...(projection.follows_active ? {} : { continuation_id: projection.viewed_continuation_id }),
    projection_revision: projection.projection_revision };
}

export async function adjacentPromptVersion(target: ThreadReadTarget, prompt: ThreadPromptSummary,
  projection: ThreadContinuationProjection, direction: 'previous' | 'next', signal?: AbortSignal): Promise<ThreadPromptVersionsPage> {
  const captured = captureThreadRead(target);
  if (captured.scope.projection === null || !sameThreadProjection(captured.scope.projection, projection)) throw threadReadChanged();
  const page = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/inputs/{input_id}/versions', {
    params: { path: { id: captured.threadId, input_id: prompt.input_id }, query: {
      ...threadReadSelector(captured), limit: 1,
      ...(direction === 'previous' ? { before: prompt.ordinal } : { after: prompt.ordinal }),
    } }, signal: readSignal,
  })), signal);
  receiveThreadRead(captured, page.projection, signal);
  if (page.group_id !== prompt.group_id || page.rows.length > 1 || page.rows.some((row) => row.group_id !== prompt.group_id
    || (direction === 'previous' ? row.ordinal >= prompt.ordinal : row.ordinal <= prompt.ordinal))) {
    throw new Error('The reply did not identify the adjacent message version');
  }
  return page;
}

/** Preserve the complete body, including client_id, when retrying an unknown outcome. */
export async function createPromptVersion(target: ThreadReadTarget, inputId: string, body: ThreadPromptVersionCreateBody): Promise<ThreadContinuationAccepted> {
  const captured = captureThreadRead(target);
  if (captured.scope.projection?.viewed_continuation_id !== body.source_continuation_id) throw threadReadChanged();
  const reply = await readInThread(captured, () => ok(api.POST('/threads/{id}/inputs/{input_id}/versions', {
    params: { path: { id: captured.threadId, input_id: inputId } }, body,
  })));
  return stampThreadReceipt(reply, captured);
}

/** A 202 receipt reports preparation, not successful delivery or a new running turn. */
export async function activateContinuation(target: ThreadReadTarget, continuationId: string, body: ThreadContinuationActivateBody): Promise<ThreadContinuationAccepted> {
  const captured = captureThreadRead(target);
  if (captured.scope.projection?.viewed_continuation_id !== continuationId) throw threadReadChanged();
  const reply = await readInThread(captured, () => ok(api.POST('/threads/{id}/continuations/{continuation_id}/activate', {
    params: { path: { id: captured.threadId, continuation_id: continuationId } }, body,
  })));
  return stampThreadReceipt(reply, captured);
}
