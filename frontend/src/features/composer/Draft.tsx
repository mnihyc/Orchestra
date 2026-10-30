/** Per-user server-synchronized draft.
 *
 * What a person is writing to the agent of a thread, and what has been sent but not yet answered.
 *
 * Typing is local first: a key is on screen at once and nothing waits for the server. The text is kept
 * for the thread while the page is open, saved to the server a second after the last key (so that it is
 * there on any device), and put into the box again when the thread is opened with an empty one. A message
 * that was sent is shown as "Sending…" until the server has answered; one the server refused goes back
 * into the box with the refusal, and one that got no answer stays with "Not sent · Try again", for
 * sending it again is safe: it carries the value that makes a repeat make nothing twice.
 */
import { useMutation, useQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { create } from 'zustand';

import { ApiError, client } from '@/api/client';
import { draftQuery, saveDraft, sendInput } from '@/api/threads';
import { clearDraftText } from '@/api/source';
import type { ContentPart, Delivery, LimitBypassRequest } from '@/generated/api-types';
import { isForwarded } from '@/lib/content';
import { NetworkError, newActKey } from '@/lib/requests';
import { threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';

/** How long after the last key the draft is saved: a pause in typing, not a rhythm of it. */
export const DRAFT_SAVE_AFTER_MS = 1_000;

// A lane and its panel may share one draft. Serialize differing writes and coalesce the same
// confirmed content, including a clear after sending, without changing the typing debounce.
interface DraftWrite { key: string; request: Promise<void>; settled: boolean }
const draftWrites = new Map<string, DraftWrite>();
const draftWriters = new Map<string, Map<symbol, () => void>>();
let draftLifetime = 0;
client.onSession((event) => {
  if (event.type !== 'signed_in' && event.type !== 'signed_out') return;
  draftLifetime += 1;
  draftWrites.clear();
});

/** One visible editor owns the debounce even when its lane and panel are both mounted. */
function useDraftWriter(threadId: string, enabled: boolean): boolean {
  const [owner] = useState(() => Symbol('draft writer'));
  const subscribe = useCallback((notify: () => void) => {
    if (!enabled) return () => undefined;
    // A confirmed write lives with the locally retained draft. Moving its editor to a panel
    // must not save those same words again from an older query-cache baseline.
    if (!draftWriters.has(threadId) && useComposerStore.getState().text[threadId] === undefined && draftWrites.get(threadId)?.settled === true) draftWrites.delete(threadId);
    const writers = draftWriters.get(threadId) ?? new Map<symbol, () => void>();
    draftWriters.set(threadId, writers);
    writers.set(owner, notify);
    for (const changed of writers.values()) changed();
    return () => {
      writers.delete(owner);
      if (writers.size === 0) {
        draftWriters.delete(threadId);
      } else for (const changed of writers.values()) changed();
    };
  }, [threadId, enabled, owner]);
  const snapshot = useCallback(() => enabled && draftWriters.get(threadId)?.keys().next().value === owner, [threadId, enabled, owner]);
  return useSyncExternalStore(subscribe, snapshot);
}

function writeDraftOnce(threadId: string, content: ContentPart[], delivery: Delivery): Promise<void> {
  const clear = content.length === 0;
  const key = clear ? 'cleared' : JSON.stringify({ content, delivery });
  const previous = draftWrites.get(threadId);
  if (previous?.key === key) return previous.request;
  const lifetime = draftLifetime;
  const write = async () => {
    // A queued write from the previous account must never use the next account's credentials.
    if (lifetime !== draftLifetime) return;
    if (clear) await clearDraftText(threadId);
    else await saveDraft(threadId, content, delivery);
  };
  const request = previous === undefined ? write() : previous.request.catch(() => undefined).then(write);
  const pending = { key, request, settled: false };
  draftWrites.set(threadId, pending);
  void request.then(() => {
    pending.settled = true;
    if (!draftWriters.has(threadId) && useComposerStore.getState().text[threadId] === undefined && draftWrites.get(threadId) === pending) draftWrites.delete(threadId);
  }, () => {
    if (draftWrites.get(threadId) === pending) draftWrites.delete(threadId);
  });
  return request;
}

export interface SendingInput {
  /** The actual execution history when this local attempt was authored; older clients omit it. */
  continuationId?: string | null;
  /** The value that makes sending it again safe. */
  clientId: string;
  content: ContentPart[];
  delivery: Delivery;
  bypass?: LimitBypassRequest | undefined;
  /** Worked in plan mode. */
  plan?: boolean;
  /** Under way, or sent and not answered. */
  state: 'sending' | 'not_sent';
  /** When it was sent. */
  at: string;
}

interface ComposerStore {
  /** Limits the person has said the next message may go past, per thread. */
  passNext: Record<string, LimitBypassRequest>;
  text: Record<string, string>;
  parts: Record<string, ContentPart[]>;
  /** What the server refused the last send with, per thread, to be read beside the box. */
  refused: Record<string, unknown>;
  sending: Record<string, SendingInput[]>;
  /** The agents that answered a steer with "cannot": asked once, remembered while the page is open. */
  cannotSteer: Record<string, true>;
  /** The composer's Plan switch per thread, where the person set it; unset follows `thread.plan_first`. */
  planning: Record<string, boolean>;
}

export const useComposerStore = create<ComposerStore>(() => ({ passNext: {}, text: {}, parts: {}, refused: {}, sending: {}, cannotSteer: {}, planning: {} }));

const NOBODY: SendingInput[] = [];
const NOPARTS: ContentPart[] = [];

/** The ready image and file parts kept in a thread's draft. */
export function useAttachmentParts(threadId: string): ContentPart[] {
  return useComposerStore((state) => state.parts[threadId] ?? NOPARTS);
}

/** The messages of a thread that were sent and not answered. */
export function useSending(threadId: string): SendingInput[] {
  return useComposerStore((state) => state.sending[threadId] ?? NOBODY);
}

function setSending(threadId: string, change: (list: SendingInput[]) => SendingInput[]): void {
  useComposerStore.setState((state) => ({ sending: { ...state.sending, [threadId]: change(state.sending[threadId] ?? []) } }));
}

export const composerWriter = {
  /** The person lets the next message of the thread go past these limits. */
  letNextPass(threadId: string, bypass: LimitBypassRequest | undefined): void {
    useComposerStore.setState((state) => {
      const { [threadId]: _gone, ...rest } = state.passNext;
      return { passNext: bypass === undefined ? rest : { ...rest, [threadId]: bypass } };
    });
  },
  setText(threadId: string, text: string): void {
    useComposerStore.setState((state) => ({ text: { ...state.text, [threadId]: text } }));
  },
  /** An acknowledged aside clears only its unchanged words, never a newer draft or newly attached parts. */
  clearAcceptedAside(threadId: string, words: string): void {
    useComposerStore.setState((state) => {
      if (state.text[threadId] !== words || (state.parts[threadId]?.length ?? 0) > 0) return state;
      return { text: { ...state.text, [threadId]: '' } };
    });
  },
  /** Keeps ready file and image parts in this thread's draft. */
  setParts(threadId: string, parts: ContentPart[]): void {
    useComposerStore.setState((state) => ({ parts: { ...state.parts, [threadId]: parts } }));
  },
  addPart(threadId: string, part: ContentPart): void {
    useComposerStore.setState((state) => ({ parts: { ...state.parts, [threadId]: [...(state.parts[threadId] ?? []), part] } }));
  },
  /** Puts words into the box as a quotation after what is there, and an empty line under it for the person's own. */
  quote(threadId: string, words: string): void {
    const quoted = words
      .trim()
      .split('\n')
      .map((line) => (line.trim() === '' ? '>' : `> ${line}`))
      .join('\n');
    useComposerStore.setState((state) => {
      const before = state.text[threadId] ?? '';
      return { text: { ...state.text, [threadId]: `${before.trim() === '' ? '' : `${before.trimEnd()}\n\n`}${quoted}\n\n` } };
    });
  },
  /** The agent said that it takes no steering: the menu offers none from now on. */
  noteCannotSteer(agentId: string): void {
    useComposerStore.setState((state) => ({ cannotSteer: { ...state.cannotSteer, [agentId]: true } }));
  },
  /** The person turns the composer's Plan switch of a thread on or off. */
  setPlanning(threadId: string, on: boolean): void {
    useComposerStore.setState((state) => ({ planning: { ...state.planning, [threadId]: on } }));
  },
  clearRefusal(threadId: string): void {
    useComposerStore.setState((state) => {
      const { [threadId]: _gone, ...refused } = state.refused;
      return { refused };
    });
  },
};

/** The text of the box of a thread, and the refusal of the last send. */
export function useBox(threadId: string): { text: string; setText: (text: string) => void; refusal: unknown } {
  const text = useComposerStore((state) => state.text[threadId] ?? '');
  const refusal = useComposerStore((state) => state.refused[threadId]);
  return { text, setText: useCallback((next: string) => composerWriter.setText(threadId, next), [threadId]), refusal };
}

/** Sends a message. The box has been emptied by the caller; a refusal gives the text back. */
export function submitInput(target: ThreadReadTarget, message: string | ContentPart[], delivery: Delivery, bypass?: LimitBypassRequest, clientId: string = newActKey(), plan = false): void {
  const read = typeof target === 'string' ? defaultThreadRead(target) : target;
  const threadId = read.threadId;
  const lifetime = draftLifetime;
  const content: ContentPart[] = typeof message === 'string' ? [{ type: 'text', text: message }] : message;
  const text = content.filter((part) => !isForwarded(part) && part.type === 'text').map((part) => 'text' in part ? part.text : '').join('\n');
  const continuation = threadViewOf(read).read?.projection?.viewed_continuation_id;
  const entry: SendingInput = { clientId, content, delivery, bypass, ...(plan ? { plan } : {}), state: 'sending', at: new Date().toISOString(),
    ...(continuation === undefined ? {} : { continuationId: continuation }) };
  setSending(threadId, (list) => [...list.filter((held) => held.clientId !== clientId), entry]);
  composerWriter.clearRefusal(threadId);
  sendInput(target, { content, delivery, clientId, ...(bypass === undefined ? {} : { bypass }), ...(plan ? { plan } : {}) }).then(
    (reply) => {
      if (lifetime !== draftLifetime) return;
      // The answer is taken like a row that came by the socket.
      threadWriter.apply(read, { inputs: [reply.input] });
      setSending(threadId, (list) => list.filter((held) => held.clientId !== clientId));
      const current = useComposerStore.getState();
      if ((current.text[threadId] ?? '') === '' && (current.parts[threadId]?.length ?? 0) === 0) {
        void writeDraftOnce(threadId, [], 'queue').catch(() => undefined);
      }
    },
    (error: unknown) => {
      if (lifetime !== draftLifetime) return;
      if (error instanceof ApiError) {
        // Refused: nothing was sent. The words go back into the box, and the refusal is read beside it.
        setSending(threadId, (list) => list.filter((held) => held.clientId !== clientId));
        const agentId = threadViewOf(read).thread?.binding?.agent.id;
        if (delivery === 'steer' && error.is('capability.unsupported') && agentId !== undefined) composerWriter.noteCannotSteer(agentId);
        useComposerStore.setState((state) => ({
          refused: { ...state.refused, [threadId]: error },
          text: { ...state.text, [threadId]: (state.text[threadId] ?? '') === '' ? text : `${text}\n${state.text[threadId] ?? ''}` },
          parts: { ...state.parts, [threadId]: [...content.filter((part) => !isForwarded(part) && part.type !== 'text'), ...(state.parts[threadId] ?? [])] },
        }));
      } else if (error instanceof NetworkError) {
        // No answer: it may or may not have arrived. It stays, and sending it again is safe.
        setSending(threadId, (list) => list.map((held) => (held.clientId === clientId ? { ...held, state: 'not_sent' } : held)));
      } else {
        setSending(threadId, (list) => list.map((held) => (held.clientId === clientId ? { ...held, state: 'not_sent' } : held)));
      }
    },
  );
}

/** Sends again a message that got no answer, with the same value, so that it cannot be made twice. */
export function resend(target: ThreadReadTarget, entry: SendingInput): void {
  const projection = threadViewOf(target).read?.projection;
  if (entry.continuationId !== undefined && entry.continuationId !== null && projection?.viewed_continuation_id !== entry.continuationId) return;
  submitInput(target, entry.content, entry.delivery, entry.bypass, entry.clientId, entry.plan === true);
}

/** Gives up a message that got no answer: its words go back into the box. */
export function discard(threadId: string, entry: SendingInput): void {
  setSending(threadId, (list) => list.filter((held) => held.clientId !== entry.clientId));
  const first = entry.content[0];
  for (const part of entry.content) if (!isForwarded(part) && part.type !== 'text') composerWriter.addPart(threadId, part);
  if (first !== undefined && !isForwarded(first) && first.type === 'text') {
    useComposerStore.setState((state) => ({ text: { ...state.text, [threadId]: first.text + (state.text[threadId] ?? '') } }));
  }
}

export type DraftStatus = 'idle' | 'saving' | 'failed';

/**
 * Keeps the box and the server's draft together: the draft is put into an empty box once when the
 * thread is opened, and the box is saved a second after the last key.
 */
export function useDraftSync(threadId: string, enabled: boolean, delivery: Delivery, reading = true): DraftStatus {
  const writes = useDraftWriter(threadId, enabled);
  const text = useComposerStore((state) => state.text[threadId] ?? '');
  const parts = useComposerStore((state) => state.parts[threadId] ?? NOPARTS);
  const draft = useQuery({ ...draftQuery(threadId), enabled: enabled && reading, retry: false });
  const restored = useRef<string | null>(null);
  const saved = useRef(JSON.stringify({ text, parts }));
  const currentDelivery = useRef(delivery);
  useEffect(() => { currentDelivery.current = delivery; }, [delivery]);
  useEffect(() => {
    const lifetime = draftLifetime;
    return () => {
      // The last editor may leave before its typing debounce fires (for example, to browse an edit).
      // Flush only its restored dirty draft and only for the same signed-in reader. A remaining
      // editor keeps the existing debounce, and writeDraftOnce coalesces simultaneous releases.
      if (!enabled || lifetime !== draftLifetime || restored.current !== threadId || draftWriters.has(threadId)) return;
      const local = useComposerStore.getState();
      const words = local.text[threadId] ?? '', attached = local.parts[threadId] ?? NOPARTS;
      if (JSON.stringify({ text: words, parts: attached }) === saved.current) return;
      const content: ContentPart[] = [...(words === '' ? [] : [{ type: 'text' as const, text: words }]), ...attached];
      void writeDraftOnce(threadId, content, currentDelivery.current).catch(() => undefined);
    };
  }, [enabled, threadId]);

  useEffect(() => {
    if (!draft.isSuccess || restored.current === threadId) return;
    restored.current = threadId;
    const first = draft.data?.content.find((part) => part.type === 'text');
    // An explicitly emptied local draft is still known: a cached older server answer must
    // not put an accepted message (or removed files) back when the composer remounts.
    if (useComposerStore.getState().text[threadId] === undefined) {
      composerWriter.setText(threadId, first?.text ?? '');
    }
    if (useComposerStore.getState().parts[threadId] === undefined) {
      composerWriter.setParts(threadId, draft.data?.content.filter((part) => !isForwarded(part) && part.type !== 'text') ?? []);
    }
    saved.current = JSON.stringify({ text: first?.text ?? '', parts: draft.data?.content.filter((part) => !isForwarded(part) && part.type !== 'text') ?? [] });
  }, [draft.isSuccess, draft.data, threadId]);

  const save = useMutation({
    mutationFn: (value: { text: string; parts: ContentPart[] }) => writeDraftOnce(threadId, value.text.trim() === '' && value.parts.length === 0 ? [] : [...(value.text === '' ? [] : [{ type: 'text' as const, text: value.text }]), ...value.parts], delivery),
  });
  const { mutate } = save;
  useEffect(() => {
    // Only what was typed after the draft was restored is saved: the restored text is the server's already.
    const key = JSON.stringify({ text, parts });
    if (!writes || restored.current !== threadId || key === saved.current) return;
    const timer = setTimeout(() => {
      saved.current = key;
      mutate({ text, parts });
    }, DRAFT_SAVE_AFTER_MS);
    return () => clearTimeout(timer);
  }, [text, parts, writes, threadId, mutate]);

  return save.isPending ? 'saving' : save.isError ? 'failed' : 'idle';
}
