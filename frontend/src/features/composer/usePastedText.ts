/** Folded paste drafts share one uploader and keep edits local until their replacement blob is ready. */
import { useCallback, useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { useStore } from 'zustand';
import { client } from '@/api/client';

import { insertAtSelection } from '@/lib/drop';
import { attachPastedText, forgetPaste, INLINE_PASTE_MAX_CHARS, pastedFile, pasteDisplayKey, pasteKey, pasteWouldScroll, readPastedText, rememberedPaste, type PastedTextPart } from '@/lib/pastedText';
import { countTextCharacters } from '@/lib/tokenEstimate';
import { releaseUpload } from '@/lib/uploads';
import { useUploads } from '@/lib/useUploads';
import { useViewWork } from '@/features/workbench/viewOpening';

interface Edit { original: PastedTextPart; text?: string; revision: number }
interface Pending { owner: symbol; revision: number; ids: string[]; stop: (id: string) => Promise<boolean> }
// Large-file counts arrive with attachment completion; the queue never blocks on their full text.
function inlineCounts(text: string): { characters?: number } {
  if (text.length > INLINE_PASTE_MAX_CHARS) return {};
  const counts = countTextCharacters(text);
  return { characters: counts.ascii + counts.other };
}
class Edits {
  readonly entries = new Map<string, Edit>();
  readonly pending = new Map<string, Pending>();
  readonly removed = new Set<string>();
  readonly identities = new Map<string, string>();
  readonly deliveries = new Map<string, 'inline' | 'file'>();
  private readonly listeners = new Set<() => void>();
  revision = 0;
  closed = false;
  readonly subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  readonly snapshot = () => this.revision;
  changed() { this.revision += 1; for (const listener of this.listeners) listener(); }
  open() { this.closed = false; }
  identity(paste: PastedTextPart) { const key = pasteDisplayKey(paste); return this.identities.get(key) ?? key; }
  stop(key: string) { const job = this.pending.get(key); this.pending.delete(key); if (job !== undefined) for (const id of job.ids) void job.stop(id); }
  clear(close = false) { this.closed = close; for (const key of this.pending.keys()) this.stop(key); this.entries.clear(); this.identities.clear(); this.deliveries.clear(); this.changed(); }
}
const scopes = new Map<string, Edits>();
function editsAt(key: string): Edits { let edits = scopes.get(key); if (edits === undefined) { edits = new Edits(); scopes.set(key, edits); } return edits; }
client.session.subscribe((next, before) => { if (next.readerVersion !== before.readerVersion) { for (const edits of scopes.values()) edits.clear(true); scopes.clear(); } });
if (typeof window !== 'undefined') window.addEventListener('beforeunload', (event) => { if ([...scopes.values()].some((edits) => edits.entries.size > 0)) event.preventDefault(); });

/** A remote draft cannot replace a person's text while a folded attachment is being edited locally. */
export function hasPastedTextEdits(kind: 'thread' | 'chat', id: string): boolean {
  return (scopes.get(`${client.session.getState().readerVersion ?? 0}:${kind}:${id}:draft`)?.entries.size ?? 0) > 0;
}

export function usePastedText({ kind, id, box, maxLines, text, setText, add, replace, remove, temporary = false }: {
  kind: 'thread' | 'chat'; id: string; box: React.RefObject<HTMLTextAreaElement | null>; maxLines: number;
  text: string; setText: (text: string) => void; add: (part: PastedTextPart) => void;
  replace: (original: PastedTextPart, part: PastedTextPart) => void; remove: (index: number) => void; temporary?: boolean;
}) {
  const reader = useStore(client.session, (state) => state.readerVersion);
  const instance = useId();
  const scope = `${reader ?? 0}:${kind}:${id}:${temporary ? instance : 'draft'}`;
  const edits = useMemo(() => editsAt(scope), [scope]);
  const revision = useSyncExternalStore(edits.subscribe, edits.snapshot);
  const [owner] = useState(() => Symbol('paste editor'));
  const active = useRef(false);
  const callbacks = useRef({ add, replace });
  useEffect(() => { callbacks.current = { add, replace }; }, [add, replace]);
  const queue = useUploads<PastedTextPart>(scope, () => Promise.reject(new Error('Paste missing')), (part) => callbacks.current.add(part));
  const { add: queueFiles, remove: stop } = queue;
  const upload = useCallback((words: string, original?: PastedTextPart) => {
    if (client.session.getState().readerVersion !== reader) return;
    const key = original === undefined ? null : pasteKey(original);
    const held = key === null ? undefined : edits.entries.get(key);
    if (key !== null && (held === undefined || edits.removed.has(key))) return;
    if (key !== null && edits.pending.get(key)?.revision === held?.revision) return;
    if (key !== null) edits.stop(key);
    const file = original === undefined ? pastedFile(words) : new File([words], original.name, { type: 'text/plain' });
    const ids = queueFiles([file], original === undefined ? undefined : (part) => {
      if (key === null || edits.entries.get(key)?.revision !== held?.revision) { void releaseUpload(part.blob).catch(() => undefined); return; }
      // A sent paste gets a new private path once; saving it must not unmount its active editor.
      const identity = edits.identity(original);
      edits.identities.set(pasteDisplayKey(part), identity);
      if (pasteDisplayKey(part) !== pasteDisplayKey(original)) edits.identities.delete(pasteDisplayKey(original));
      const delivery = edits.deliveries.get(identity) ?? original.delivery;
      callbacks.current.replace(original, { ...part, ...(delivery === undefined ? {} : { delivery }) });
      edits.deliveries.delete(identity);
      edits.entries.delete(key); edits.pending.delete(key); edits.changed();
      if (part.blob !== original.blob) void releaseUpload(original.blob, kind === 'thread' ? { thread: id, path: original.path } : undefined).catch(() => undefined);
    }, (blob, signal) => attachPastedText(kind, id, blob, words, signal, original?.path), { kind: 'pasted_text', ...inlineCounts(words), ...(key === null ? {} : { replaces: key }) });
    if (key !== null && held !== undefined) { edits.pending.set(key, { owner, revision: held.revision, ids, stop }); edits.changed(); }
  }, [reader, edits, queueFiles, stop, kind, id, owner]);
  useEffect(() => {
    active.current = true;
    edits.open();
    return () => {
      active.current = false;
      for (const [key, job] of edits.pending) if (job.owner === owner) edits.stop(key);
      if (temporary) edits.clear(true); else edits.changed();
    };
  }, [edits, owner, temporary]);
  useEffect(() => {
    for (const [key, edit] of edits.entries) if (edit.text !== undefined && !edits.pending.has(key)) upload(edit.text, edit.original);
  }, [edits, revision, upload]);
  useViewWork(edits.entries.size > 0);
  const markDirty = (original: PastedTextPart, changed: boolean) => {
    const key = pasteKey(original);
    if (edits.closed || edits.removed.has(key) || client.session.getState().readerVersion !== reader) return;
    if (!changed) { edits.entries.delete(key); edits.stop(key); }
    else edits.entries.set(key, { original, revision: (edits.entries.get(key)?.revision ?? 0) + 1 });
    edits.changed();
  };
  const edit = (original: PastedTextPart, words: string) => {
    const key = pasteKey(original);
    if (edits.closed || edits.removed.has(key) || client.session.getState().readerVersion !== reader) return;
    if (words === rememberedPaste(original.blob)) { edits.entries.delete(key); edits.stop(key); edits.changed(); return; }
    const held = edits.entries.get(key);
    edits.entries.set(key, { original, text: words, revision: held?.revision ?? 1 }); edits.changed();
    if (active.current) upload(words, original);
  };
  const cancel = (paste: PastedTextPart) => {
    const key = pasteKey(paste); edits.removed.add(key); edits.stop(key); edits.entries.delete(key); edits.deliveries.delete(edits.identity(paste)); edits.identities.delete(pasteDisplayKey(paste)); edits.changed(); forgetPaste(paste.blob);
  };
  const paste = (words: string): boolean => {
    const element = box.current;
    if (element === null || words === '') return false;
    const next = insertAtSelection(text, words, element);
    if (!pasteWouldScroll(element, next.text, maxLines)) return false;
    const start = element.selectionStart, end = element.selectionEnd;
    setText(`${text.slice(0, start)}${text.slice(end)}`);
    upload(words); return true;
  };
  const asText = (index: number, words: string) => {
    const next = insertAtSelection(text, words, box.current);
    remove(index); setText(next.text);
    requestAnimationFrame(() => { box.current?.focus(); box.current?.setSelectionRange(next.caret, next.caret); });
  };
  const delivery = (paste: PastedTextPart, value: 'inline' | 'file') => {
    if (edits.entries.has(pasteKey(paste))) edits.deliveries.set(edits.identity(paste), value);
    callbacks.current.replace(paste, { ...paste, delivery: value });
  };
  const pendingText = {
    dirty: (uploadId: string) => queue.pause(uploadId),
    edit: (uploadId: string, words: string) => {
      const row = queue.rows.find((row) => row.id === uploadId);
      if (row === undefined || client.session.getState().readerVersion !== reader) return;
      queue.revise(uploadId, new File([words], row.file.name, { type: 'text/plain' }), (blob, signal) => attachPastedText(kind, id, blob, words, signal), { kind: 'pasted_text', ...inlineCounts(words) });
    },
    asText: (uploadId: string, words: string) => {
      void queue.remove(uploadId);
      const next = insertAtSelection(text, words, box.current);
      setText(next.text);
      requestAnimationFrame(() => { box.current?.focus(); box.current?.setSelectionRange(next.caret, next.caret); });
    },
  };
  return { queue: { ...queue, paste: pendingText }, paste, edit, cancel, asText, delivery, markDirty, editing: (paste: PastedTextPart) => edits.entries.has(pasteKey(paste)), editText: (paste: PastedTextPart) => edits.entries.get(pasteKey(paste))?.text,
    identity: (paste: PastedTextPart) => edits.identity(paste), read: (paste: PastedTextPart) => readPastedText(kind, id, paste), blocked: queue.blocked || edits.entries.size > 0 };
}
