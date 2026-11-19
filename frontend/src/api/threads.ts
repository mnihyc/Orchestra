/** API client for Timeline, input, side threads, binding, exports, what the caller lacks to give input, and the limits in a turn's way with letting an input past them.
 *
 * What the screen of a thread asks of the API: the whole of a thread or what changed since a
 * sequence, older items and a stretch around one, one item and the tail of a command's output; what
 * is said to the agent, changed in the queue, taken back, and the turn stopped; the limits that stand
 * in a turn's way; pinning; the draft kept for the composer; how far the person has read; the model
 * and effort the thread is set to; and making a thread in a topic.
 *
 * Every request that creates carries the key of the act, and an input carries its own `client_id`, so
 * that a second attempt after a lost answer makes nothing twice. A change to something with a
 * revision names the revision the person saw.
 */
import { treeWriter } from '@/realtime/stores/tree';
import { infiniteQueryOptions, queryOptions, type QueryClient } from '@tanstack/react-query';

import { api, client, done, ok, repeatable } from '@/api/client';
import { applyThread, isCompleteThread } from '@/api/cache';
import { queryClient } from '@/api/query';
import { settingsKey } from '@/api/settings';
import { assertThreadRead, captureThreadExecution, captureThreadRead, isCurrentThreadRead, readInThread,
  receiveThreadRead, stampThreadRows, threadAct, threadQueryRead, threadReadQueryKey, threadReadSelector, type ThreadReadCapture } from '@/api/threadRead';
import { isDefaultThreadRead, sameThreadRead, threadReadTarget, type ThreadReadTarget, planThreadOpening, scopedThreadKey, stampThreadReceipt, threadReadChanged } from '@/realtime/threadScope';
export { captureThreadRead, isCurrentThreadRead, selectThreadContinuation, threadReadQueryKey, threadReadSelector } from '@/api/threadRead';
import type {
  Catchup,
  ContentPart,
  CreateThread,
  Delivery,
  Draft,
  Item,
  LimitBypassRequest,
  NodeSettingSetBody,
  ThreadOutlinePage,
  ThreadSegmentPage,
  ThreadTurnPage,
  ResolvedSetting,
  Thread,
  ThreadInput,
  ThreadPlanDecideBody,
  ThreadPlanDecideReply,
  ThreadInputCreateReply,
  ThreadInputUpdateReply,
  ThreadItemListReply,
  ThreadLimits,
  ThreadOpening,
  ThreadParameters,
  ThreadQueue,
  ThreadSync,
  Turn,
} from '@/generated/api-types';
import { THREAD_INPUT_PAGE_SIZE, threadViewOf, threadWriter } from '@/realtime/stores/thread';

export const threadKeys = {
  parameters: (id: string) => ['thread-parameters', id] as const,
  draft: (id: string) => ['thread-draft', id] as const,
  limits: (id: string) => ['thread-limits', id] as const,
  catchup: (target: ThreadReadTarget, since: number) => threadReadQueryKey(target, ['thread-catchup', threadReadTarget(target).threadId, since]),
  outline: (target: ThreadReadTarget) => threadReadQueryKey(target, ['thread-outline', threadReadTarget(target).threadId]),
};

/** How many items the first answer holds: a screenful and then some, so that the first scroll up needs no request. */
export const TAIL_ITEMS = 200;
/** How many older items a scroll to the top reads. */
export const OLDER_ITEMS = 100;

/** Install the screen's native projections before its thread is published to mounted controls. */
export function hydrateThreadOpening(cache: QueryClient, opening: ThreadOpening, captured?: ThreadReadCapture, outlineKey?: readonly unknown[]): void {
  if (!isCompleteThread(opening.sync.thread)) throw new Error('The opening did not carry a complete thread view');
  const id = opening.sync.thread.id;
  if (captured !== undefined) {
    assertThreadRead(captured);
    if (captured.threadId !== id) throw threadReadChanged();
    if (!isDefaultThreadRead(captured.ref) && (captured.ref.controls !== true || captured.scope.selection !== null)) {
      // Read-only previews publish history only; full content tabs share the normal controls.
      cache.setQueryData(outlineKey ?? threadKeys.outline(captured.ref), { pages: [opening.outline], pageParams: [null] });
      return;
    }
  }
  // The composer is not a history page; switching a read must not replace a dirty cached draft.
  if (captured === undefined || isDefaultThreadRead(captured.ref) && !captured.scope.switched && captured.scope.selection === null
    || cache.getQueryState(threadKeys.draft(id)) === undefined) cache.setQueryData(threadKeys.draft(id), opening.draft);
  if (opening.parameters !== null) cache.setQueryData(threadKeys.parameters(id), opening.parameters);
  else cache.removeQueries({ queryKey: threadKeys.parameters(id), exact: true });
  if (opening.limits !== null) cache.setQueryData([...threadKeys.limits(id), null], opening.limits);
  else cache.removeQueries({ queryKey: [...threadKeys.limits(id), null], exact: true });
  cache.setQueryData(settingsKey(id), opening.settings);
  cache.setQueryData(outlineKey ?? threadKeys.outline(id), { pages: [opening.outline], pageParams: [null] });
  if (opening.sync.projection === undefined || opening.sync.projection === null || opening.sync.projection.follows_active) {
    applyThread(cache, opening.sync.thread);
  }
}

/** The whole thread (`since` is `null`: its newest part), or what changed after a sequence. */
export async function fetchThreadSync(target: ThreadReadTarget, since: number | null, signal?: AbortSignal): Promise<ThreadSync> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  if (since === null) {
    const captured = threadWriter.beginRead(ref, client.session.getState().readerVersion);
    const opening = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/opening', {
      params: { path: { id }, query: { limit: TAIL_ITEMS, ...threadReadSelector(captured, false) } },
      signal: readSignal,
    })), signal, true);
    if (opening.sync.thread.id !== id) throw new Error('The opening belongs to a different thread');
    if (!isCompleteThread(opening.sync.thread)) throw new Error('The opening did not carry a complete thread view');
    if (!opening.sync.complete) throw new Error('The full thread opening was incomplete');
    const planned = planThreadOpening(captured.scope, opening.sync.projection ?? null);
    if (planned === null) throw threadReadChanged();
    hydrateThreadOpening(queryClient, opening, captured, scopedThreadKey(planned, ['thread-outline', id]));
    const accepted = threadWriter.reset(ref, opening.sync, opening.sync.has_older, captured, planned);
    if (accepted === null) throw threadReadChanged();
    return stampThreadRows(opening.sync, accepted);
  }
  const captured = captureThreadRead(ref);
  const reply = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/sync', {
    params: { path: { id }, query: { since, limit: TAIL_ITEMS, ...threadReadSelector(captured) } },
    signal: readSignal,
  })), signal);
  if (!isCompleteThread(reply.thread) || reply.thread.id !== id) {
    const error = new Error('The sync did not carry this complete thread view');
    threadWriter.refused(ref,error,captured);
    throw error;
  }
  const accepted = receiveThreadRead(captured, reply.projection, signal);
  return stampThreadRows(reply, accepted);
}

/** Items before a sequence, after it, or around an item. */
export type ItemWindow = ({ before: number } | { after: number } | { around: string } | { turn: number }) & { stream?: string };

export async function fetchItems(target: ThreadReadTarget, window: ItemWindow, limit: number = OLDER_ITEMS, signal?: AbortSignal): Promise<ThreadItemListReply> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const captured = captureThreadRead(ref);
  const reply = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/items', {
    params: { path: { id }, query: { ...window, limit, ...threadReadSelector(captured) } },
    signal: readSignal,
  })), signal);
  const accepted = receiveThreadRead(captured, reply.projection, signal);
  stampThreadRows(reply, accepted);
  threadWriter.apply(ref, { turns: reply.turns, segments: reply.segments }, accepted);
  return reply;
}

/** A child stream's newest page, or the page before its oldest displayed sequence. */
export function fetchSubagentItems(target: ThreadReadTarget, stream: string, before: number, signal?: AbortSignal): Promise<ThreadItemListReply> {
  return fetchItems(target, { stream, before }, OLDER_ITEMS, signal);
}

export async function fetchItem(target: ThreadReadTarget, itemId: string, signal?: AbortSignal, captured = captureThreadRead(target)): Promise<Item> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  if (!sameThreadRead(captured.ref,ref)) throw threadReadChanged();
  const item = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/items/{item_id}', {
    params: { path: { id, item_id: itemId }, query: threadReadSelector(captured) },
    signal: readSignal,
  })), signal);
  return stampThreadReceipt(item, captured);
}

/** A stretch of a command's whole output, as text, under the same captured history. */
export function fetchOutput(target: ThreadReadTarget, itemId: string, offset: number, limit: number, signal?: AbortSignal,
  captured = captureThreadRead(target)): Promise<string> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  if (!sameThreadRead(captured.ref,ref)) return Promise.reject(threadReadChanged());
  return readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/items/{item_id}/output', {
    params: { path: { id, item_id: itemId }, query: { offset, limit, ...threadReadSelector(captured) } },
    parseAs: 'text', signal: readSignal,
  })), signal);
}

/** Counts and pages bind the query key and its request to the same immutable local read ticket. */
export const catchupQuery = (target: ThreadReadTarget, since: number) => {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-catchup', id, since]);
  return queryOptions({
    queryKey: reading.key,
    enabled: reading.enabled,
    queryFn: async ({ signal }): Promise<Catchup> => {
      const captured = await reading.capture(signal);
      const page = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/catchup', {
        params: { path: { id }, query: { since, ...threadReadSelector(captured) } }, signal: readSignal,
      })), signal);
      const accepted = receiveThreadRead(captured, page.projection, signal);
      return stampThreadReceipt(page, accepted);
    },
    staleTime: Infinity,
  });
};

export const outlineQuery = (target: ThreadReadTarget) => {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-outline', id]);
  return infiniteQueryOptions({
    queryKey: reading.key,
    enabled: reading.enabled,
    initialPageParam: null as number | null,
    queryFn: async ({ signal, pageParam }): Promise<ThreadOutlinePage> => {
      const captured = await reading.capture(signal, pageParam !== null);
      const page = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/outline', {
        params: { path: { id }, query: { limit: TAIL_ITEMS, ...threadReadSelector(captured),
          ...(pageParam === null ? {} : { before: pageParam }) } }, signal: readSignal,
      })), signal);
      const accepted = receiveThreadRead(captured, page.projection, signal);
      return stampThreadReceipt(page, accepted);
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
    staleTime: Infinity,
  });
};

/** Historical references remain explicitly pageable, even for turns with no timeline items. */
export const segmentsQuery = (target: ThreadReadTarget) => {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-segments', id]);
  return infiniteQueryOptions({
    queryKey: reading.key,
    enabled: reading.enabled,
    initialPageParam: null as number | null,
    queryFn: async ({ signal, pageParam }): Promise<ThreadSegmentPage> => {
      const captured = await reading.capture(signal, pageParam !== null);
      const page = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/segments', {
        params: { path: { id }, query: { limit: OLDER_ITEMS, ...threadReadSelector(captured),
          ...(pageParam === null ? {} : { before: pageParam }) } }, signal: readSignal,
      })), signal);
      const accepted = receiveThreadRead(captured, page.projection, signal);
      stampThreadRows(page, accepted);
      threadWriter.apply(ref, { segments: page.segments }, accepted);
      return page;
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
    staleTime: Infinity,
  });
};

export const turnsQuery = (target: ThreadReadTarget) => {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const reading = threadQueryRead(ref, ['thread-turns', id]);
  return infiniteQueryOptions({
    queryKey: reading.key,
    enabled: reading.enabled,
    initialPageParam: null as number | null,
    queryFn: async ({ signal, pageParam }): Promise<ThreadTurnPage> => {
      const captured = await reading.capture(signal, pageParam !== null);
      const page = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/turns', {
        params: { path: { id }, query: { limit: OLDER_ITEMS, ...threadReadSelector(captured),
          ...(pageParam === null ? {} : { before: pageParam }) } }, signal: readSignal,
      })), signal);
      const accepted = receiveThreadRead(captured, page.projection, signal);
      stampThreadRows(page, accepted);
      threadWriter.apply(ref, { turns: page.turns, segments: page.segments }, accepted);
      return page;
    },
    getNextPageParam: (page) => page.next_before ?? undefined,
    staleTime: Infinity,
  });
};

/** Explicit lazy queue demand only; never walk delivered inputs or automatically drain pages. */
export function loadNextThreadInputs(target: ThreadReadTarget, signal?: AbortSignal): Promise<void> {
  return loadThreadInputs(target, false, signal);
}

/** A completed pending edit/cancel refreshes only its owning queue, with the original view ticket. */
export async function refreshThreadInputs(captured: ThreadReadCapture): Promise<void> {
  assertThreadRead(captured);
  return loadThreadInputs(captured.ref, true);
}

async function loadThreadInputs(target: ThreadReadTarget, restart: boolean, signal?: AbortSignal): Promise<void> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const captured = captureThreadRead(ref);
  const held = threadViewOf(ref).queuePage;
  if (!restart && (held.loading || held.revision === null || held.nextAfter === null && !held.restartRequired)) return;
  if (captured.scope.projection === null) throw threadReadChanged();
  const first = restart || held.restartRequired;
  const request = {};
  threadWriter.queueLoading(ref, captured, request);
  try {
    const page = await readInThread(captured, (readSignal) => ok(api.GET('/threads/{id}/inputs', {
      params: { path: { id }, query: { limit: THREAD_INPUT_PAGE_SIZE, ...threadReadSelector(captured),
        ...(first ? {} : { after: held.nextAfter, queue_revision: held.revision }) } },
      signal: readSignal,
    })), signal);
    const accepted = receiveThreadRead(captured, page.projection, signal);
    if (!first && (page.queue_revision !== held.revision || threadViewOf(ref).queuePage.revision !== held.revision)) throw threadReadChanged();
    threadWriter.queueLoaded(ref, page, accepted, first, request);
  } catch (error) {
    if (isCurrentThreadRead(captured)) {
      if (signal?.aborted === true) threadWriter.queueCancelled(ref, captured, request);
      else threadWriter.queueFailed(ref, error, captured, request);
    }
    throw error;
  }
}

// ---------------------------------------------------------------- inputs

export interface InputBody {
  content: ContentPart[];
  delivery: Delivery;
  /** The value that makes sending it again safe. */
  clientId: string;
  bypass?: LimitBypassRequest;
  /** Worked in plan mode: the agent proposes a plan and changes nothing until somebody proceeds. */
  plan?: boolean;
}

/** Says something to the agent: queued, steered into the turn that runs, or sent over it. */
export function sendInput(target: ThreadReadTarget, body: InputBody): Promise<ThreadInputCreateReply> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(
    api.POST('/threads/{id}/inputs', {
      params: { path: { id } },
      body: {
        content: body.content,
        delivery: body.delivery,
        client_id: body.clientId,
        ...(body.bypass === undefined ? {} : { bypass: body.bypass }),
        ...(body.plan === true ? { plan: true } : {}),
      },
    }),
  ), (reply) => [reply.input]);
}

/** Changes a waiting input; the answer includes every neighbor renumbered by the same act. */
export async function updateInput(
  target: ThreadReadTarget,
  inputId: string,
  body: { content?: ContentPart[]; afterInputId?: string | null; delivery?: Delivery },
): Promise<ThreadInputUpdateReply> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const contentOnly = body.content !== undefined && body.afterInputId === undefined && body.delivery === undefined;
  const captured = contentOnly ? captureThreadRead(ref) : captureThreadExecution(ref);
  const reply = await readInThread(captured, () => ok(
    api.PATCH('/threads/{id}/inputs/{input_id}', {
      params: { path: { id, input_id: inputId } },
      body: {
        ...(body.content === undefined ? {} : { content: body.content }),
        ...(body.afterInputId === undefined ? {} : { after_input_id: body.afterInputId }),
        ...(body.delivery === undefined ? {} : { delivery: body.delivery }),
      },
    }),
  ));
  // The server's neighbor patch is for selected execution, never an inactive queue.
  const scoped = captured.scope.selection === null ? reply : { ...reply, inputs: [] };
  stampThreadRows({ inputs: [...scoped.inputs, scoped.input] }, captured);
  return stampThreadReceipt(scoped, captured);
}

/** Removes pending words by their immutable input ID without activating their continuation. */
export async function cancelInput(target: ThreadReadTarget, inputId: string): Promise<ThreadInput> {
  const captured = captureThreadRead(target);
  const reply = await readInThread(captured, () => ok(api.DELETE('/threads/{id}/inputs/{input_id}', {
    params: { path: { id: captured.threadId, input_id: inputId } },
  })));
  return stampThreadReceipt(reply, captured);
}

/** Stops a turn. */
export function interruptTurn(target: ThreadReadTarget, turnId: string): Promise<Turn> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(api.POST('/threads/{id}/interrupt', { params: { path: { id } }, body: { turn_id: turnId } })));
}

/** Tries again, for its author, the files that hold an input: the same input, held again if they still cannot go. */
export function retryInput(target: ThreadReadTarget, inputId: string): Promise<ThreadInput> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(api.POST('/threads/{id}/inputs/{input_id}/retry', { params: { path: { id, input_id: inputId } } })));
}

/** Resumes what a Stop held, naming the revision of the queue that was seen. */
export function resumeQueue(target: ThreadReadTarget, revision: number): Promise<ThreadQueue> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(api.POST('/threads/{id}/queue/resume', { params: { path: { id } }, body: { revision } })));
}

/** Queues a copy of the input of a turn that failed or was stopped. */
export function retryTurn(target: ThreadReadTarget, turnId: string, clientId: string, bypass?: LimitBypassRequest): Promise<ThreadInputCreateReply> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(
    api.POST('/threads/{id}/turns/{turn_id}/retry', {
      params: { path: { id, turn_id: turnId } },
      body: { client_id: clientId, ...(bypass === undefined ? {} : { bypass }) },
    }),
  ), (reply) => [reply.input]);
}

/** Lets a waiting input past the limits that hold it. */
export function bypassInput(target: ThreadReadTarget, inputId: string, body: LimitBypassRequest): Promise<ThreadInput> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(api.POST('/threads/{id}/inputs/{input_id}/bypass', { params: { path: { id, input_id: inputId } }, body })));
}

/** Runs a message that waits for somebody who may use the thread's model on the caller's own access. */
export function runInput(target: ThreadReadTarget, inputId: string): Promise<ThreadInput> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  return threadAct(ref, () => ok(api.POST('/threads/{id}/inputs/{input_id}/run', { params: { path: { id, input_id: inputId } } })));
}

/** Answers a proposed plan once under the same current read and execution fence as an input. */
export function decidePlan(target:ThreadReadTarget,itemId:string,body:ThreadPlanDecideBody):Promise<ThreadPlanDecideReply> {
  const ref=threadReadTarget(target);
  return threadAct(ref,()=>ok(api.POST('/threads/{id}/plans/{item_id}/decision',{params:{path:{id:ref.threadId,item_id:itemId}},body})),(reply)=>[reply.input]);
}

/** The limits that stand in the way of the next turn, and who may let it past. */
export const limitsQuery = (id: string, inputId?: string) =>
  queryOptions({
    queryKey: [...threadKeys.limits(id), inputId ?? null] as const,
    queryFn: ({ signal }): Promise<ThreadLimits> =>
      ok(api.GET('/threads/{id}/limits', { params: { path: { id }, query: inputId === undefined ? {} : { input_id: inputId } }, signal })),
    staleTime: Infinity,
  });

export async function pinItem(target: ThreadReadTarget, itemId: string, pinned: boolean): Promise<Item> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const captured = captureThreadExecution(ref);
  const row = await readInThread(captured, () => pinned
    ? ok(api.PUT('/threads/{id}/items/{item_id}/pin', { params: { path: { id, item_id: itemId } } }))
    : ok(api.DELETE('/threads/{id}/items/{item_id}/pin', { params: { path: { id, item_id: itemId } } })));
  const held = threadViewOf(ref).items.get(itemId);
  const projection = captured.scope.projection;
  if (projection !== null && row.continuation_id !== projection.viewed_continuation_id && held === undefined) {
    // This answer has no frozen payload to preserve. Repair only this exact admitted item,
    // never a whole list, instead of publishing the former owner's current body.
    return fetchItem(ref, itemId, undefined, captured);
  }
  // Native inherited reads freeze bodies while pin/quote metadata remains shared. An ordinary
  // pin receipt cannot replace that frozen payload with the former owner's current body.
  const projected = projection !== null && held !== undefined && held.continuation_id !== projection.viewed_continuation_id
    ? { ...held, pinned: row.pinned, chat_refs: row.chat_refs, seq_updated: row.seq_updated, updated_at: row.updated_at } : row;
  return stampThreadReceipt(projected, captured);
}

// ---------------------------------------------------------------- the draft and the read position

/** The draft kept for the person on the server, or nothing. */
export const draftQuery = (id: string) =>
  queryOptions({
    queryKey: threadKeys.draft(id),
    queryFn: ({ signal }): Promise<Draft | null> => ok(api.GET('/threads/{id}/draft', { params: { path: { id } }, signal })),
    staleTime: Infinity,
  });

export function saveDraft(id: string, content: ContentPart[], delivery: Delivery): Promise<Draft> {
  return ok(api.PUT('/threads/{id}/draft', { params: { path: { id } }, body: { content, delivery } }));
}

export function deleteDraft(id: string): Promise<void> {
  return done(api.DELETE('/threads/{id}/draft', { params: { path: { id } } }));
}

/** Says how far the person has read; the stored value is the larger of the old and the new. */
export async function markRead(target: ThreadReadTarget, seq: number): Promise<number> {
  const ref = threadReadTarget(target);
  const id = ref.threadId;
  const captured = captureThreadRead(ref);
  const reply = await readInThread(captured, (readSignal) => ok(api.POST('/threads/{id}/read', {
    params: { path: { id } }, body: { seq, ...threadReadSelector(captured) }, signal: readSignal,
  })));
  const accepted = receiveThreadRead(captured, reply.projection);
  threadWriter.readStateAll(id, accepted.scope.projection?.viewed_continuation_id ?? null, reply.last_read_seq);
  if (reply.thread_display !== null && reply.thread_display !== undefined) { threadWriter.display(id, reply.thread_display); treeWriter.display(id, reply.thread_display); }
  return reply.last_read_seq;
}

// ---------------------------------------------------------------- the model, the effort and the settings

/** What can be set on the agent of a thread, and what it is now. */
export const parametersQuery = (id: string) =>
  queryOptions({
    queryKey: threadKeys.parameters(id),
    queryFn: ({ signal }): Promise<ThreadParameters> => ok(api.GET('/threads/{id}/parameters', { params: { path: { id } }, signal })),
    staleTime: Infinity,
  });

/** Writes one setting of the thread itself: the model and the effort are two of them, and apply from the next turn. */
export function setThreadSetting(id: string, key: string, body: NodeSettingSetBody): Promise<ResolvedSetting> {
  return ok(api.PUT('/nodes/{id}/settings/{key}', { params: { path: { id, key } }, body }));
}

/** Gives the title or the description back to automation. */
export function resumeAutomatic(id: string, field: 'title' | 'description', action: 'resume' | 'refresh' = 'resume'): Promise<Thread> {
  return ok(api.POST('/nodes/{id}/auto', { params: { path: { id } }, body: { field, action } })).then((node) => {
    if (node.kind !== 'thread') throw new Error('the node is not a thread');
    return node;
  });
}

// ---------------------------------------------------------------- making a thread

/** Makes a thread in a topic. The key of the act is made once, when the person acts. */
export function createThread(topicId: string, body: CreateThread, key: string): Promise<Thread> {
  return ok(api.POST('/topics/{id}/threads', { params: { path: { id: topicId } }, body, ...repeatable(key) }));
}
