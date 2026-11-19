/** Asking on the side: the entry every screen calls, and the one request behind it.
 *
 * A side thread is a quick aside that a person takes from a thread: a short conversation with the same agent set-up,
 * which starts from the thread's history and adds nothing to it. It is started from the send menu of the box (with
 * what is written there), or from a message of the conversation (which takes the history up to it, and asks for the
 * one line to say). The host that stands in the frame draws the small dialog; the request is made here, once, with a
 * value of the act's own that makes sending it again safe.
 */
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { create } from 'zustand';

import { forkKeys, startSide } from '@/api/forks';
import { ApiError, client } from '@/api/client';
import { deleteNode } from '@/api/nodes';
import { newActKey } from '@/lib/requests';
import type { Thread } from '@/generated/api-types';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { captureThreadExecution, captureThreadRead, isCurrentThreadRead } from '@/api/threadRead';
import { useThreadReadRef } from '@/realtime/threadReadContext';
import { sameThreadRead, threadReadChanged, threadReadTarget, type ThreadReadCapture, type ThreadReadRef, type ThreadReadTarget } from '@/realtime/threadScope';
import { timelineUi } from '@/features/timeline/state';

/** What is asked for: the thread, the turn whose end the side thread begins from (the last completed one when left out), and a line to start with. */
export interface SideAsk {
  threadId: string;
  threadName: string;
  toTurn: number | null;
  text: string;
  /** Local opener only; native requests still address threadId. */
  read?: ThreadReadRef;
}

export interface OpenedSideAsk extends SideAsk { origin?: ThreadReadCapture }

function asideScope(target: ThreadReadTarget, toTurn: number | null): string {
  const read = threadReadTarget(target);
  return JSON.stringify(read.key.kind === 'thread' ? [read.threadId, toTurn] : [read.threadId, toTurn, read.key.kind, read.key.owner]);
}

interface Question { text: string; toTurn: number | null }
type SideReply = Awaited<ReturnType<typeof startSide>>;
interface Acceptance { reply: SideReply; question: Question; lifetime: number; reader: number | undefined; origin: ThreadReadCapture }
interface Attempt {
  key: string;
  origin: ThreadReadCapture;
  question: Question;
  request: Promise<Acceptance> | null;
  error: unknown;
}

interface HostState {
  current: OpenedSideAsk | null;
  open: boolean;
  /** Changes only for a deliberately different aside; closing keeps its draft and retry identity. */
  instance: number;
  /** Unanswered acts survive a consumer leaving the page, within this signed-in lifetime. */
  attempts: Readonly<Record<string, Attempt>>;
  lifetime: number;
  show: (ask: SideAsk) => void;
  hide: () => void;
  forget: () => void;
}

export const useSideAsk = create<HostState>((set, get) => ({
  current: null,
  open: false,
  instance: 0,
  attempts: {},
  lifetime: 0,
  show: (ask) => {
    const state = get();
    const current = state.current;
    const read = threadReadTarget(ask.read ?? ask.threadId);
    if (read.threadId !== ask.threadId) throw threadReadChanged();
    const origin = captureThreadRead(read);
    const unanswered = state.attempts[asideScope(read, ask.toTurn)] !== undefined;
    const same = current !== null && current.threadId === ask.threadId && current.toTurn === ask.toTurn
      && sameThreadRead(current.read ?? current.threadId, read)
      && (current.origin === undefined || isCurrentThreadRead(current.origin))
      && (unanswered || ask.text === '' || current.text === ask.text);
    const opened = { ...ask, read, origin };
    set(same ? { current: { ...current, ...opened, text: current.text }, open: true } : { current: opened, open: true, instance: get().instance + 1 });
  },
  // Keep this form's draft when closed; unresolved acts also survive a page or form replacement.
  hide: () => set({ open: false }),
  forget: () => set({ current: null, open: false, attempts: {}, lifetime: get().lifetime + 1 }),
}));

// A retained question belongs to this signed-in lifetime, never to the next person at the device.
client.onSession((event) => {
  if (event.type === 'signed_out' || event.type === 'signed_in') useSideAsk.getState().forget();
});

/** Opens the one-line dialog that starts a side thread of a thread. */
export function openSideAsk(ask: SideAsk): void {
  useSideAsk.getState().show(ask);
}

/**
 * Starts a side thread of a thread with a message in it. The value that makes sending again safe is kept until the
 * act has succeeded, so that a second press after a lost answer makes one side thread and not two.
 */
export function useAskOnSide(target: ThreadReadTarget, { toTurn = null, origin, onAccepted }: {
  toTurn?: number | null;
  origin?: ThreadReadCapture;
  onAccepted?: (reply: SideReply, question: Question) => void;
} = {}) {
  const queryClient = useQueryClient();
  const read = useThreadReadRef(target);
  const scope = asideScope(read, toTurn);
  const attempt = useSideAsk((state) => state.attempts[scope]);
  const lifetime = useSideAsk((state) => state.lifetime);
  const mutation = useMutation({
    mutationKey: ['side-ask', lifetime, scope],
    mutationFn: ({ text }: Question) => requestAside(read, scope, { text, toTurn }, origin),
    // Hook-level acceptance also runs after navigation unmounts the caller. A late answer from
    // a previous account lifetime must never clear the new person's draft or close their dialog.
    onSuccess: (accepted) => {
      if (useSideAsk.getState().lifetime !== accepted.lifetime || accepted.reader !== client.session.getState().readerVersion) return;
      const { thread, input } = accepted.reply;
      threadWriter.apply(thread.id, { thread, inputs: [input] });
      // A returned side can update a complete held list, but cannot stand for its older siblings.
      queryClient.setQueryData<Thread[]>(forkKeys.sides(accepted.origin.threadId, 'open'), (held) => held === undefined ? undefined : [...held.filter((side) => side.id !== thread.id), thread]);
      const sameCaller = sameThreadRead(read, accepted.origin.ref) && toTurn === accepted.question.toTurn;
      if (sameCaller && isCurrentThreadRead(accepted.origin)) timelineUi.openSide(accepted.origin.ref, thread.id);
      if (sameCaller) onAccepted?.(accepted.reply, accepted.question);
    },
  });
  return {
    mutate: mutation.mutate,
    reset: mutation.reset,
    isPending: mutation.isPending || (attempt !== undefined && attempt.request !== null),
    unconfirmed: attempt !== undefined && asideUnconfirmed(attempt.error),
    error: attempt?.error ?? mutation.error,
    variables: attempt?.question ?? mutation.variables,
  };
}

/** An absent or server-error answer does not prove that the aside was not made. */
export function asideUnconfirmed(error: unknown): boolean {
  return error !== null && error !== undefined && (!(error instanceof ApiError) || error.status >= 500);
}

/** One in-flight request per thread/history boundary, or a retry of its frozen unresolved act. */
function requestAside(read: ThreadReadRef, scope: string, question: Question, expected?: ThreadReadCapture): Promise<Acceptance> {
  const threadId = read.threadId;
  if (expected !== undefined && !sameThreadRead(expected.ref, read)) throw threadReadChanged();
  const state = useSideAsk.getState();
  const held = state.attempts[scope];
  if (held !== undefined && held.request !== null) return held.request;
  if (held === undefined && (threadViewOf(read).error !== null || expected !== undefined && !isCurrentThreadRead(expected))) throw threadReadChanged();
  const attempt: Attempt = held ?? { key: newActKey(), origin: captureThreadExecution(read), question: { ...question }, request: null, error: null };
  const lifetime = state.lifetime, reader = client.session.getState().readerVersion;
  const finish = (error: unknown) => useSideAsk.setState((current) => {
    if (current.lifetime !== lifetime || current.attempts[scope]?.key !== attempt.key) return current;
    const { [scope]: _gone, ...rest } = current.attempts;
    return { attempts: asideUnconfirmed(error) ? { ...rest, [scope]: { ...attempt, request: null, error } } : rest };
  });
  const request = startSide(threadId, {
    content: [{ type: 'text', text: attempt.question.text }], client_id: attempt.key,
    ...(attempt.question.toTurn === null ? {} : { to_turn: attempt.question.toTurn }),
  }).then((reply) => {
    if (reply.thread.id === threadId || reply.thread.side_of?.id !== threadId || reply.input.thread_id !== reply.thread.id) {
      throw new Error('The accepted side belongs to a different thread or input');
    }
    finish(null);
    return { reply, question: attempt.question, lifetime, reader, origin: attempt.origin };
  }).catch((error: unknown) => {
    finish(error);
    throw error;
  });
  useSideAsk.setState((current) => current.lifetime === lifetime ? { attempts: { ...current.attempts, [scope]: { ...attempt, request } } } : current);
  return request;
}

/** Closes a side thread: by its maker, or by whoever manages its thread. It folds to one line in the conversation. */
export function useCloseSide(threadId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (sideId: string) => deleteNode(sideId),
    onSuccess: (_reply, sideId) => {
      threadWriter.closed(sideId, 'deleted');
      queryClient.setQueryData<Thread[]>(forkKeys.sides(threadId, 'open'), (held) => held?.filter((side) => side.id !== sideId));
    },
  });
}
