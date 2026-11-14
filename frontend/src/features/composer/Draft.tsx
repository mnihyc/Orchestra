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

import { ApiError, client, ReceiptAcknowledged } from '@/api/client';
import { draftQuery, saveDraft, sendInput } from '@/api/threads';
import { clearDraftText } from '@/api/source';
import type { ContentPart, Delivery, LimitBypassRequest, NativeInvocation } from '@/generated/api-types';
import { i18n } from '@/i18n';
import { isForwarded } from '@/lib/content';
import { NetworkError, newActKey } from '@/lib/requests';
import { runningTurn, threadViewOf, threadWriter } from '@/realtime/stores/thread';
import { defaultThreadRead, type ThreadReadTarget } from '@/realtime/threadScope';
import { toast } from '@/ui/Toast';

/** How long after the last key the draft is saved: a pause in typing, not a rhythm of it. */
export const DRAFT_SAVE_AFTER_MS = 1_000;

// A lane and its panel may share one draft. Serialize differing writes and coalesce the same
// confirmed content, including a clear after sending, without changing the typing debounce.
interface DraftWrite { key: string; request: Promise<void>; settled: boolean }
const draftWrites = new Map<string, DraftWrite>();
const draftWriters = new Map<string, Map<symbol, () => void>>();
const draftNotices = new Map<string, () => void>();
let draftLifetime = 0;
client.onSession((event) => {
  if (event.type !== 'signed_in' && event.type !== 'signed_out') return;
  draftLifetime += 1;
  draftWrites.clear();
  for (const dismiss of draftNotices.values()) dismiss();
  draftNotices.clear();
  useComposerStore.setState({ nativeSelection: {}, draftSaves: {}, dirtyDrafts: new Set() });
});

/** One visible editor owns the debounce even when its lane and panel are both mounted. */
function useDraftWriter(threadId: string, enabled: boolean, storeKey: string): boolean {
  const [owner] = useState(() => Symbol('draft writer'));
  const subscribe = useCallback((notify: () => void) => {
    if (!enabled) return () => undefined;
    // A confirmed write lives with the locally retained draft. Moving its editor to a panel
    // must not save those same words again from an older query-cache baseline.
    if (!draftWriters.has(threadId) && useComposerStore.getState().text[storeKey] === undefined && draftWrites.get(threadId)?.settled === true) draftWrites.delete(threadId);
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
  }, [threadId, enabled, owner, storeKey]);
  const snapshot = useCallback(() => enabled && draftWriters.get(threadId)?.keys().next().value === owner, [threadId, enabled, owner]);
  return useSyncExternalStore(subscribe, snapshot);
}

function writeDraftOnce(threadId: string, content: ContentPart[], delivery: Delivery, storeKey = threadId, nativeAction?: NativeInvocation | null, stillCurrent?: () => boolean): Promise<void> {
  const clear = content.length === 0;
  const key = clear && nativeAction === undefined ? 'cleared' : JSON.stringify({ content, delivery, nativeAction });
  const previous = draftWrites.get(threadId);
  if (previous?.key === key) return previous.request;
  const lifetime = draftLifetime;
  const name = threadViewOf(threadId).thread?.name ?? '';
  draftSaveState(threadId, storeKey, 'saving');
  let acknowledged = false;
  const write = async () => {
    // A queued write from the previous account must never use the next account's credentials.
    if (lifetime !== draftLifetime || stillCurrent?.() === false) return;
    try {
      if (clear && nativeAction === undefined) await clearDraftText(threadId);
      else if (nativeAction === undefined) await saveDraft(threadId, content, delivery);
      else await saveDraft(threadId, content, delivery, nativeAction);
    } catch (error) {
      if (!(error instanceof ReceiptAcknowledged)) throw error;
    }
    acknowledged = true;
  };
  const request = previous === undefined ? write() : previous.request.catch(() => undefined).then(write);
  const pending = { key, request, settled: false };
  draftWrites.set(threadId, pending);
  void request.then(() => {
    if (lifetime !== draftLifetime) return;
    if (!acknowledged) {
      if (draftWrites.get(threadId) === pending) { draftWrites.delete(threadId); draftSaveState(threadId, storeKey, 'idle'); }
      return;
    }
    pending.settled = true;
    const confirmed = draftContentKey(content);
    const latest = draftWrites.get(threadId) === pending;
    draftSaveState(threadId, storeKey, latest ? 'idle' : undefined, null, confirmed);
    if (latest) {
      draftNotices.get(storeKey)?.();
      draftNotices.delete(storeKey);
    }
    if (!draftWriters.has(threadId) && useComposerStore.getState().text[storeKey] === undefined && draftWrites.get(threadId) === pending) draftWrites.delete(threadId);
  }, (error: unknown) => {
    if (lifetime !== draftLifetime || draftWrites.get(threadId) !== pending) return;
    draftWrites.delete(threadId);
    draftSaveState(threadId, storeKey, 'failed', error);
    if (!draftWriters.has(threadId)) {
      draftNotices.get(storeKey)?.();
      const dismiss = toast({ tone: 'danger', title: name === '' ? i18n.t('composer.draft.failed') : i18n.t('composer.draft.failedNamed', { name }), description: i18n.t('composer.draft.kept'),
        action: { label: i18n.t('common.tryAgain'), run: () => {
          if (lifetime !== draftLifetime) return;
          const current = useComposerStore.getState().draftSaves[storeKey];
          if (current?.status !== 'failed') return;
          draftNotices.get(storeKey)?.();
          draftNotices.set(storeKey, toast({ tone: 'info', title: i18n.t('composer.draft.saving'), persistent: true }));
          void persistComposerDraft(threadId, storeKey).catch(() => undefined);
        } } });
      draftNotices.set(storeKey, dismiss);
    }
  });
  return request;
}

/** Saves the current private draft through the shared writer, including attachment completion and retries. */
export function persistComposerDraft(threadId: string, storeKey = threadId, attached?: ContentPart): Promise<void> {
  const local = useComposerStore.getState(), text = local.text[storeKey] ?? '';
  const content: ContentPart[] = [...(text === '' ? [] : [{ type: 'text', text } as const]), ...local.parts[storeKey] ?? [], ...(attached === undefined ? [] : [attached])];
  return writeDraftOnce(threadId, content, 'queue', storeKey);
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
  /** A known null prevents an accepted native choice returning from an older cached draft. */
  nativeSelection: Record<string, NativeInvocation | null>;
  /** What the server refused the last send with, per thread, to be read beside the box. */
  refused: Record<string, unknown>;
  sending: Record<string, SendingInput[]>;
  /** The turn that refused steering, per thread; a later turn does not inherit the refusal. */
  cannotSteer: Record<string, string>;
  /** The composer's Plan switch per thread, where the person set it; unset follows `thread.plan_first`. */
  planning: Record<string, boolean>;
  /** Save acknowledgement belongs to the private draft, including when its last editor has left. */
  draftSaves: Record<string, { threadId: string; status: DraftStatus; error: unknown; confirmed: string | null }>;
  dirtyDrafts: ReadonlySet<string>;
}

export const useComposerStore = create<ComposerStore>(() => ({ passNext: {}, text: {}, parts: {}, nativeSelection: {}, refused: {}, sending: {}, cannotSteer: {}, planning: {}, draftSaves: {}, dirtyDrafts: new Set() }));

const draftKeyOf = (text: string, parts: readonly ContentPart[]): string => JSON.stringify({ text: parts.length === 0 && text.trim() === '' ? '' : text, parts });
const draftContentKey = (content: readonly ContentPart[]): string => draftKeyOf(content.filter((part) => !isForwarded(part) && part.type === 'text').map((part) => 'text' in part ? part.text : '').join('\n'), content.filter((part) => !isForwarded(part) && part.type !== 'text'));

function markDraftDirty(storeKey: string, dirty: boolean): void {
  useComposerStore.setState((state) => {
    if (state.dirtyDrafts.has(storeKey) === dirty) return state;
    const dirtyDrafts = new Set(state.dirtyDrafts);
    if (dirty) dirtyDrafts.add(storeKey); else dirtyDrafts.delete(storeKey);
    return { dirtyDrafts };
  });
}

function draftSaveState(threadId: string, storeKey: string, status?: DraftStatus, error: unknown = null, confirmed?: string): void {
  useComposerStore.setState((state) => {
    const held = state.draftSaves[storeKey];
    if (held !== undefined && held.threadId !== threadId && status === undefined) return state;
    const before = held?.threadId === threadId ? held : undefined;
    const next: ComposerStore['draftSaves'][string] = { threadId, status: status ?? before?.status ?? 'idle', error: status === undefined ? before?.error ?? null : error, confirmed: confirmed ?? before?.confirmed ?? null };
    const dirtyDrafts = new Set(state.dirtyDrafts);
    if (next.status === 'saving' || next.status === 'failed' || draftKeyOf(state.text[storeKey] ?? '', state.parts[storeKey] ?? []) !== next.confirmed) dirtyDrafts.add(storeKey);
    else dirtyDrafts.delete(storeKey);
    return { draftSaves: { ...state.draftSaves, [storeKey]: next }, dirtyDrafts };
  });
}

// Uploads and quotes can change a private draft after its last editor has left.
useComposerStore.subscribe((state, before) => {
  if (state.text === before.text && state.parts === before.parts) return;
  const keys = new Set([...Object.keys(state.text), ...Object.keys(before.text), ...Object.keys(state.parts), ...Object.keys(before.parts)]);
  const dirtyDrafts = new Set(state.dirtyDrafts);
  let changed = false;
  for (const key of keys) {
    if (state.text[key] === before.text[key] && state.parts[key] === before.parts[key]) continue;
    const held = Object.hasOwn(state.text, key) || Object.hasOwn(state.parts, key);
    const saved = state.draftSaves[key];
    const text = state.text[key] ?? '', parts = state.parts[key] ?? NOPARTS;
    const dirty = held && (saved === undefined ? text.trim() !== '' || parts.length > 0
      : saved.status === 'saving' || saved.status === 'failed' || draftKeyOf(text, parts) !== saved.confirmed);
    if (dirtyDrafts.has(key) === dirty) continue;
    if (dirty) dirtyDrafts.add(key); else dirtyDrafts.delete(key);
    changed = true;
  }
  if (changed) useComposerStore.setState({ dirtyDrafts });
});

/** The shell protects unsaved private words even after the editor is paused or removed. */
export function useComposerDraftGuard(): void {
  const dirty = useComposerStore((state) => state.dirtyDrafts.size > 0);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
}

/** A learned refusal belongs only to the active turn that actually returned it. */
export function steerRefusedFor(state: Pick<ComposerStore, 'cannotSteer'>, threadId: string, turnId: string | null): boolean {
  return turnId !== null && state.cannotSteer[threadId] === turnId;
}

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
  /** A schedule accepts this exact frozen draft, never another edit or a later reader's draft. */
  clearScheduledDraft(threadId: string, words: string, parts: readonly ContentPart[], stillCurrent: () => boolean): boolean {
    const local = useComposerStore.getState();
    if (!stillCurrent() || (local.text[threadId] ?? '') !== words || JSON.stringify(local.parts[threadId] ?? []) !== JSON.stringify(parts)) return false;
    useComposerStore.setState((state) => ({ text: { ...state.text, [threadId]: '' }, parts: { ...state.parts, [threadId]: [] }, nativeSelection: { ...state.nativeSelection, [threadId]: null } }));
    void writeDraftOnce(threadId, [], 'queue', threadId, null, stillCurrent).catch(() => undefined);
    return true;
  },
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
  /** This turn said it takes no steering; a new turn may report a different capability. */
  noteCannotSteer(threadId: string, turnId: string): void {
    useComposerStore.setState((state) => ({ cannotSteer: { ...state.cannotSteer, [threadId]: turnId } }));
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

/** A delayed refusal can describe only the same live turn in its original read scope. */
export function noteSteerRefusal(target: ThreadReadTarget, turnId: string | null): void {
  if (turnId === null) return;
  const current = threadViewOf(target);
  if (current.closed === null && (current.read === null || current.read.ready)
    && (runningTurn(current)?.id ?? current.thread?.current_turn_id ?? null) === turnId) {
    composerWriter.noteCannotSteer(typeof target === 'string' ? target : target.threadId, turnId);
  }
}

/** Sends a message. The box has been emptied by the caller; a refusal gives the text back. */
export function submitInput(target: ThreadReadTarget, message: string | ContentPart[], delivery: Delivery, bypass?: LimitBypassRequest, clientId: string = newActKey(), plan = false): void {
  const read = typeof target === 'string' ? defaultThreadRead(target) : target;
  const threadId = read.threadId;
  const lifetime = draftLifetime;
  const content: ContentPart[] = typeof message === 'string' ? [{ type: 'text', text: message }] : message;
  const text = content.filter((part) => !isForwarded(part) && part.type === 'text').map((part) => 'text' in part ? part.text : '').join('\n');
  const view = threadViewOf(read);
  const continuation = view.read?.projection?.viewed_continuation_id;
  const steerTurnId = delivery === 'steer' ? runningTurn(view)?.id ?? view.thread?.current_turn_id ?? null : null;
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
        if (delivery === 'steer' && error.is('capability.unsupported')) noteSteerRefusal(read, steerTurnId);
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
export function useDraftSync(threadId: string, enabled: boolean, delivery: Delivery, reading = true, storeKey = threadId, onDirty?: (dirty: boolean) => void): DraftStatus {
  const writes = useDraftWriter(threadId, enabled, storeKey);
  const text = useComposerStore((state) => state.text[storeKey] ?? '');
  const parts = useComposerStore((state) => state.parts[storeKey] ?? NOPARTS);
  const persistence = useComposerStore((state) => state.draftSaves[storeKey]);
  const identity = `${threadId}:${storeKey}`;
  const draft = useQuery({ ...draftQuery(threadId), enabled: enabled && reading, retry: false });
  const restored = useRef<string | null>(null);
  const saved = useRef<string | null>(null);
  const attempted = useRef<string | null>(null);
  const currentDelivery = useRef(delivery);
  useEffect(() => { currentDelivery.current = delivery; }, [delivery]);
  useEffect(() => {
    const lifetime = draftLifetime;
    return () => {
      // The last editor may leave before its typing debounce fires (for example, to browse an edit).
      // Flush only its restored dirty draft and only for the same signed-in reader. A remaining
      // editor keeps the existing debounce, and writeDraftOnce coalesces simultaneous releases.
      if (!enabled || lifetime !== draftLifetime || restored.current !== identity || draftWriters.has(threadId)) return;
      const local = useComposerStore.getState();
      const words = local.text[storeKey] ?? '', attached = local.parts[storeKey] ?? NOPARTS;
      if (draftKeyOf(words, attached) === saved.current && useComposerStore.getState().draftSaves[storeKey]?.status !== 'failed') return;
      const content: ContentPart[] = [...(words === '' ? [] : [{ type: 'text' as const, text: words }]), ...attached];
      void writeDraftOnce(threadId, content, currentDelivery.current, storeKey).catch(() => undefined);
    };
  }, [enabled, threadId, storeKey, identity]);

  useEffect(() => {
    if (!enabled || !draft.isSuccess || restored.current === identity) return;
    attempted.current = null;
    restored.current = identity;
    const first = draft.data?.content.find((part) => part.type === 'text');
    // An explicitly emptied local draft is still known: a cached older server answer must
    // not put an accepted message (or removed files) back when the composer remounts.
    if (useComposerStore.getState().text[storeKey] === undefined) {
      composerWriter.setText(storeKey, first?.text ?? '');
    }
    if (useComposerStore.getState().parts[storeKey] === undefined) {
      composerWriter.setParts(storeKey, draft.data?.content.filter((part) => !isForwarded(part) && part.type !== 'text') ?? []);
    }
    if (!Object.hasOwn(useComposerStore.getState().nativeSelection, storeKey)) useComposerStore.setState((state) => ({ nativeSelection: { ...state.nativeSelection, [storeKey]: draft.data?.native_action ?? null } }));
    const baseline = draftContentKey(draft.data?.content ?? []);
    const held = useComposerStore.getState().draftSaves[storeKey];
    saved.current = held?.threadId === threadId && held.confirmed !== null ? held.confirmed : baseline;
    if (held?.threadId !== threadId) draftSaveState(threadId, storeKey, 'idle', null, baseline);
  }, [enabled, draft.isSuccess, draft.data, identity, threadId, storeKey]);

  useEffect(() => {
    if (restored.current === identity && persistence?.threadId === threadId && persistence.confirmed !== null) saved.current = persistence.confirmed;
  }, [identity, persistence, threadId]);

  const { mutate } = useMutation({
    mutationFn: (value: { text: string; parts: ContentPart[]; threadId: string; storeKey: string; delivery: Delivery; identity: string; lifetime: number }) => value.lifetime !== draftLifetime ? Promise.resolve()
      : writeDraftOnce(value.threadId, value.text.trim() === '' && value.parts.length === 0 ? [] : [...(value.text === '' ? [] : [{ type: 'text' as const, text: value.text }]), ...value.parts], value.delivery, value.storeKey),
  });
  useEffect(() => {
    // Save only local differences from the restored server baseline.
    // A newly allocated conversation can already have local text and parts when its first read finishes.
    const key = draftKeyOf(text, parts);
    if (!writes || restored.current !== identity || key === saved.current || key === attempted.current) return;
    const lifetime = draftLifetime;
    const timer = setTimeout(() => {
      if (lifetime !== draftLifetime) return;
      attempted.current = key;
      draftSaveState(threadId, storeKey, 'saving');
      mutate({ text, parts, threadId, storeKey, delivery, identity, lifetime });
    }, DRAFT_SAVE_AFTER_MS);
    return () => clearTimeout(timer);
  }, [text, parts, writes, identity, threadId, storeKey, delivery, mutate, draft.isSuccess, persistence?.confirmed]);

  useEffect(() => {
    const changed = restored.current !== identity || saved.current === null ? text !== '' || parts.length > 0 : draftKeyOf(text, parts) !== saved.current;
    const dirty = changed || persistence?.status === 'saving' || persistence?.status === 'failed';
    markDraftDirty(storeKey, dirty);
    onDirty?.(dirty);
  }, [text, parts, identity, storeKey, draft.isSuccess, draft.data, onDirty, persistence]);
  return enabled && persistence?.threadId === threadId ? persistence.status : 'idle';
}
