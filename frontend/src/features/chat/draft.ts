/** The draft of a message: what is typed, the message it answers and the items it quotes, kept on the server a second after typing stops and followed to the person's other devices.
 *
 * Typing is the page's own: no key waits for an answer. What is typed is saved behind the person's
 * back, one second after the last key, and shows only as "Saving…" or "Saved" beside the text. A
 * draft that was changed on another device replaces this one when this one has not been touched
 * since it was last saved, and never when the person has typed something here that is not saved.
 */
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { create } from 'zustand';

import { discardDraft, draftKey, fetchDraft, saveDraft } from '@/api/chats';
import { client } from '@/api/client';
import { sameValue } from '@/features/settings/settingsModel';
import { draftMentionText, editedMentionNames, insertedMentionName, typedTextParts } from '@/features/people/mentionParts';
import type { NamedMention } from '@/features/people/pending';
import type { ChatDraft, ChatPart } from '@/generated/api-types';
import { isForwarded } from '@/lib/content';
import { onChatDraftChanged } from '@/realtime/stores/chat';
import { hasPastedTextEdits } from '@/features/composer/usePastedText';
import { pasteKey, type PastedTextPart } from '@/lib/pastedText';

/** How long typing rests before the draft is saved: long enough for one burst of keys to be one save. */
export const SAVE_AFTER_MS = 1_000;

export interface Quote {
  threadId: string;
  itemId: string;
}

interface Content {
  mentions?: NamedMention[];
  text: string;
  replyTo: string | null;
  quotes: Quote[];
  attachments: ChatPart[];
}

export type SaveState = 'idle' | 'saving' | 'saved' | 'failed';

export interface Draft extends Content {
  setText: (text: string) => void;
  setReplyTo: (messageId: string | null) => void;
  addQuotes: (quotes: readonly Quote[]) => void;
  removeQuote: (at: number) => void;
  addAttachment: (part: ChatPart) => void;
  removeAttachment: (at: number) => void;
  replaceAttachment?: (original: PastedTextPart, part: ChatPart) => void;
  addMention?: (mention: NamedMention, at?: number) => void;
  restore?: (parts: ChatPart[], replyTo: string | null, mentions: NamedMention[]) => void;
  /** Empties it, for a message that was sent. */
  clear: () => void;
  saving: SaveState;
  /** Repeat a failed save with the current draft, without retyping or sending it. */
  retry?: () => void;
}

const EMPTY: Content = { text: '', replyTo: null, quotes: [], attachments: [], mentions: [] };

/** What the page keeps of each chat's draft while its composer is not drawn, so that going to another chat and back loses nothing. */
const kept = new Map<string, Content>();
// Replaced/expanded composers share one write order, so an older request cannot overwrite newer words.
const writes = new Map<string, Promise<void>>();
function writeInOrder(scope: string, work: () => Promise<void>): Promise<void> {
  const next = (writes.get(scope) ?? Promise.resolve()).then(work);
  writes.set(scope, next);
  void next.finally(() => { if (writes.get(scope) === next) writes.delete(scope); });
  return next;
}
const useUnsavedDrafts = create<{ scopes: ReadonlySet<string> }>(() => ({ scopes: new Set() }));
function markUnsaved(scope: string, dirty: boolean): void {
  const before = useUnsavedDrafts.getState().scopes;
  if (before.has(scope) === dirty) return;
  const scopes = new Set(before);
  if (dirty) scopes.add(scope); else scopes.delete(scope);
  useUnsavedDrafts.setState({ scopes });
}
client.onSession((event) => {
  if (event.type !== 'signed_out') return;
  kept.clear();
  writes.clear();
  useUnsavedDrafts.setState({ scopes: new Set() });
});

/** The frame protects failed/unsaved drafts even after a chat body is paused or unmounted. */
export function useChatDraftGuard(): void {
  const dirty = useUnsavedDrafts((state) => state.scopes.size > 0);
  useEffect(() => {
    if (!dirty) return;
    const warn = (event: BeforeUnloadEvent) => event.preventDefault();
    window.addEventListener('beforeunload', warn);
    return () => window.removeEventListener('beforeunload', warn);
  }, [dirty]);
}

const key = (content: Content): string => JSON.stringify(content);
const isEmpty = (content: Content): boolean => content.text.trim() === '' && content.quotes.length === 0 && content.attachments.length === 0;

/** The parts a draft is saved and sent as: its text, then the items it quotes. */
export function partsOfDraft(content: Pick<Content, 'text' | 'quotes'> & Partial<Pick<Content, 'attachments' | 'mentions'>>): ChatPart[] {
  const quotes: ChatPart[] = content.quotes.map((quote) => ({ type: 'item_ref', thread_id: quote.threadId, item_id: quote.itemId }));
  return [...typedTextParts(content.text, content.mentions ?? []), ...quotes, ...(content.attachments ?? [])];
}

function contentOf(draft: ChatDraft | null): Content {
  if (draft === null) return EMPTY;
  const quotes: Quote[] = [];
  for (const part of draft.parts) {
    if (!isForwarded(part) && part.type === 'item_ref') quotes.push({ threadId: part.thread_id, itemId: part.item_id });
  }
  return { text: draftMentionText(draft.parts, draft.mention_names ?? []), mentions: draft.mention_names ?? [], replyTo: draft.reply_to, quotes, attachments: draft.parts.filter((part) => !isForwarded(part) && (part.type === 'image' || part.type === 'attachment' || part.type === 'pasted_text')) };
}

export function useDraft(chatId: string, writable: boolean, reading = true): Draft {
  const cache = useQueryClient();
  const reader = useRef(client.session.getState().readerVersion);
  const scope = `${client.session.getState().userId ?? ''}:${chatId}`;
  // The cache owns one read across a panel/expanded copy and transient mounts; a hidden composer reads nothing.
  const draft = useQuery({ queryKey: draftKey(chatId), queryFn: () => fetchDraft(chatId), enabled: writable && reading, staleTime: Infinity, retry: false });
  const [content, setContent] = useState<Content>(() => kept.get(scope) ?? EMPTY);
  const [saving, setSaving] = useState<SaveState>('idle');
  // What the server holds, as far as this page knows: the baseline a change is measured against.
  const saved = useRef(key(EMPTY));
  const latest = useRef(content);
  const restored = useRef<string | null>(null);
  useLayoutEffect(() => {
    latest.current = content;
  });

  const change = useCallback(
    (next: (before: Content) => Content) => {
      setContent((before) => {
        const after = next(before);
        kept.set(scope, after);
        markUnsaved(scope, key(after) !== saved.current);
        return after;
      });
    },
    [scope],
  );

  // The draft the server holds, for a composer that opens with nothing in it.
  useEffect(() => {
    if (!writable || !draft.isSuccess || hasPastedTextEdits('chat', chatId)) return;
    const theirs = contentOf(draft.data);
    const accepted = sameValue(latest.current, theirs);
    if (restored.current === chatId) {
      // Another mounted copy can acknowledge these same words. Object member order is immaterial.
      if (accepted) { saved.current = key(latest.current); markUnsaved(scope, key(kept.get(scope) ?? latest.current) !== saved.current); }
      return;
    }
    restored.current = chatId;
    saved.current = accepted ? key(latest.current) : key(theirs);
    markUnsaved(scope, !isEmpty(latest.current) && !accepted);
    if (isEmpty(latest.current) && !isEmpty(theirs)) change(() => theirs);
  }, [draft.isSuccess, draft.data, writable, change, chatId, scope]);

  // A draft changed on another device.
  useEffect(
    () => {
      if (!writable || !reading) return;
      return onChatDraftChanged((id, draft) => {
        if (id !== chatId) return;
        const accept = (incoming: ChatDraft | null) => {
          if (reader.current !== client.session.getState().readerVersion) return;
          cache.setQueryData(draftKey(chatId), incoming);
          const theirs = contentOf(incoming);
          const accepted = sameValue(latest.current, theirs);
          if (!hasPastedTextEdits('chat', chatId) && (key(latest.current) === saved.current || accepted)) {
            saved.current = accepted ? key(latest.current) : key(theirs);
            markUnsaved(scope, key(kept.get(scope) ?? latest.current) !== saved.current);
            if (!accepted) change(() => theirs);
          }
        };
        // A durable event retains stable targets; their names are projected for the reader by GET.
        if (draft?.parts.some((part) => part.type === 'mention') === true && (draft.mention_names?.length ?? 0) === 0) void cache.query({ queryKey: draftKey(chatId), queryFn: () => fetchDraft(chatId), staleTime: 0 }).then(accept).catch(() => undefined);
        else accept(draft);
      });
    },
    [chatId, change, cache, writable, reading, scope],
  );

  const flush = useCallback(
    (now: Content): Promise<void> => writeInOrder(scope, async () => {
      if (reader.current !== client.session.getState().readerVersion) return;
      // Cleanup from a retired composer never republishes words changed in its replacement.
      if (key(now) !== key(kept.get(scope) ?? latest.current)) return;
      if (key(now) === saved.current) return;
      if (!isEmpty(now)) setSaving('saving');
      try {
        let accepted: ChatDraft | null = null;
        if (isEmpty(now)) {
          if (saved.current !== key(EMPTY)) await discardDraft(chatId);
        } else {
          accepted = await saveDraft(chatId, { parts: partsOfDraft(now), reply_to: now.replyTo });
        }
        if (reader.current !== client.session.getState().readerVersion) return;
        cache.setQueryData(draftKey(chatId), accepted);
        saved.current = key(now);
        markUnsaved(scope, key(kept.get(scope) ?? latest.current) !== saved.current);
        setSaving(isEmpty(now) ? 'idle' : 'saved');
      } catch {
        if (reader.current !== client.session.getState().readerVersion) return;
        setSaving('failed');
      }
    }),
    [chatId, cache, scope],
  );

  // One second after typing stops.
  useEffect(() => {
    if (!writable || key(content) === saved.current) return;
    const timer = setTimeout(() => void flush(content), SAVE_AFTER_MS);
    return () => clearTimeout(timer);
  }, [content, writable, flush]);

  // Leaving the composer saves what is unsaved at once.
  useEffect(
    () => () => {
      if (writable) void flush(latest.current);
    },
    [writable, flush],
  );

  const setText = useCallback((text: string) => change((before) => ({ ...before, text, mentions:editedMentionNames(before.text, text, before.mentions ?? []) })), [change]);
  const setReplyTo = useCallback((replyTo: string | null) => change((before) => ({ ...before, replyTo })), [change]);
  const addQuotes = useCallback(
    (quotes: readonly Quote[]) =>
      change((before) => ({
        ...before,
        quotes: [...before.quotes, ...quotes.filter((one) => !before.quotes.some((held) => held.itemId === one.itemId))],
      })),
    [change],
  );
  const removeQuote = useCallback((at: number) => change((before) => ({ ...before, quotes: before.quotes.filter((_, index) => index !== at) })), [change]);
  const clear = useCallback(() => change(() => EMPTY), [change]);
  const addAttachment = useCallback((part: ChatPart) => change((before) => ({ ...before, attachments: [...before.attachments, part] })), [change]);
  const addMention = useCallback((mention: NamedMention, at = Number.MAX_SAFE_INTEGER) => change((before) => ({ ...before, mentions:insertedMentionName(before.text, at, mention, before.mentions ?? []) })), [change]);
  const removeAttachment = useCallback((at: number) => change((before) => ({ ...before, attachments: before.attachments.filter((_, index) => index !== at) })), [change]);
  const replaceAttachment = useCallback((original: PastedTextPart, part: ChatPart) => change((before) => ({ ...before, attachments: before.attachments.map((held) => held.type === 'pasted_text' && pasteKey(held) === pasteKey(original) ? part : held) })), [change]);
  const restore = useCallback((parts: ChatPart[], replyTo: string | null, mentions: NamedMention[]) => change(() => contentOf({ parts, reply_to:replyTo, mention_names:mentions, updated_at:new Date().toISOString() })), [change]);

  return { ...content, saving, retry: () => { if (writable) void flush(latest.current); }, setText, setReplyTo, addQuotes, removeQuote, addAttachment, removeAttachment, replaceAttachment, addMention, restore, clear };
}
