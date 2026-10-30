/** Per-person device recovery for document comments; retaining a draft never publishes it. */
import { useEffect, useMemo } from 'react';
import { createStore, useStore, type StoreApi } from 'zustand';
import { client } from '@/api/client';
import { UNSAVED_KEPT_MS } from '@/features/files/Editing';
import { useViewSaveGuard, type ViewSaveGuard } from '@/features/workbench/CloseViews';
import { allTabs } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { newActKey } from '@/lib/requests';

export type CommentTarget = { kind: 'new'; block: string; quote: string }
  | { kind: 'reply'; discussion: string } | { kind: 'edit'; discussion: string; comment: string };
export type CommentRun = { type: 'text'; text: string } | { type: 'break' } | { type: 'mention'; user_id: string };
export interface CommentMention { id: string; username: string }
export interface CommentSelection { start: number; end: number; direction: 'forward' | 'backward' | 'none' }
export interface CommentAttempt { text: string; content: CommentRun[]; clientId: string; expectedSeq: number | null }
export interface LocalComment {
  target: CommentTarget; text: string; mentions: CommentMention[]; selection: CommentSelection | null;
  attempt: CommentAttempt | null; revision: number; baseSeq: number | null; conflictSeq: number | null;
}
export interface CommentDraftData {
  version: 1; user: string; owner: string; document: string; at: number;
  compose: { block: string; quote: string } | null; focused: string | null;
  slots: Readonly<Record<string, LocalComment>>;
}
interface DraftState { data: CommentDraftData; saved: boolean; error: unknown; pending: ReadonlyMap<string, string>; forget: ReadonlySet<string>; reset: number }
export interface CommentDrafts {
  key: string; store: StoreApi<DraftState>; generation: number;
}
export interface CommentReader { user: string | null; version: number }
export interface SendingComment { reader: CommentReader; generation: number; request: string; slot: string; revision: number; attempt: CommentAttempt }

const controllers = new Map<string, CommentDrafts>();
// Volatile editor identity is separate from serialized drafts and hook-returned controllers.
const composeSources = new WeakMap<CommentDrafts, CommentDraftData['compose']>();
export const commentComposeSource = (controller: CommentDrafts): CommentDraftData['compose'] | undefined => composeSources.get(controller);
export function rememberCommentComposeSource(controller: CommentDrafts, compose: CommentDraftData['compose']): void { composeSources.set(controller, compose); }
// A forced credential retirement may have no successful device write to recover.
// Keep only failed local words, inaccessible to a different signed-in person.
const retiredFailures = new Map<string, { data: CommentDraftData; error: unknown; forget: ReadonlySet<string> }>();
const bindings = new Map<string, Set<CommentDrafts>>();
const marked = new Set<string>();
const noLossTargets: readonly string[] = [];
const lossTargets = createStore<ReadonlyMap<string, readonly string[]>>(() => new Map());
const prefix = (user: string) => `orch.${user}.unsaved.document-comments.`;
export const commentDraftKey = (user: string, owner: string, document: string): string => `${prefix(user)}${encodeURIComponent(JSON.stringify([owner, document]))}`;
export const commentSlotKey = (target: CommentTarget): string => JSON.stringify(target.kind === 'new'
  ? [target.kind, target.block, target.quote] : target.kind === 'reply' ? [target.kind, target.discussion] : [target.kind, target.discussion, target.comment]);
export const commentReader = (): CommentReader => ({ user: client.session.getState().userId, version: client.session.getState().readerVersion ?? 0 });
export const currentCommentReader = (reader: CommentReader): boolean => {
  const current = commentReader();
  return reader.user !== null && reader.user === current.user && reader.version === current.version && client.session.getState().status === 'signed_in';
};
const object = (value: unknown): value is Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value);
function targetOf(value: unknown): CommentTarget | null {
  if (!object(value)) return null;
  if (value.kind === 'new' && typeof value.block === 'string' && value.block !== '' && typeof value.quote === 'string') return { kind: 'new', block: value.block, quote: value.quote };
  if (value.kind === 'reply' && typeof value.discussion === 'string' && value.discussion !== '') return { kind: 'reply', discussion: value.discussion };
  if (value.kind === 'edit' && typeof value.discussion === 'string' && value.discussion !== '' && typeof value.comment === 'string' && value.comment !== '') return { kind: 'edit', discussion: value.discussion, comment: value.comment };
  return null;
}
function runsOf(value: unknown): CommentRun[] | null {
  if (!Array.isArray(value)) return null;
  const input: unknown[] = value;
  const runs: CommentRun[] = [];
  for (const run of input) {
    if (!object(run)) return null;
    if (run.type === 'text' && typeof run.text === 'string') runs.push({ type: 'text', text: run.text });
    else if (run.type === 'break') runs.push({ type: 'break' });
    else if (run.type === 'mention' && typeof run.user_id === 'string') runs.push({ type: 'mention', user_id: run.user_id });
    else return null;
  }
  return runs;
}
function slotOf(value: unknown): LocalComment | null {
  if (!object(value) || typeof value.text !== 'string' || typeof value.revision !== 'number' || !Number.isSafeInteger(value.revision) || value.revision < 0) return null;
  const target = targetOf(value.target);
  if (target === null || !Array.isArray(value.mentions)) return null;
  const baseSeq = value.baseSeq ?? null;
  if (baseSeq !== null && (typeof baseSeq !== 'number' || !Number.isSafeInteger(baseSeq) || baseSeq < 0)) return null;
  const conflictSeq = value.conflictSeq ?? null;
  if (conflictSeq !== null && (typeof conflictSeq !== 'number' || !Number.isSafeInteger(conflictSeq) || conflictSeq < 0)) return null;
  const mentions: CommentMention[] = [];
  const people: unknown[] = value.mentions;
  for (const person of people) {
    if (!object(person) || typeof person.id !== 'string' || typeof person.username !== 'string') return null;
    mentions.push({ id: person.id, username: person.username });
  }
  let selection: CommentSelection | null = null;
  if (value.selection !== null) {
    const selected = value.selection;
    if (!object(selected) || typeof selected.start !== 'number' || typeof selected.end !== 'number'
      || !Number.isSafeInteger(selected.start) || !Number.isSafeInteger(selected.end) || selected.start < 0
      || selected.end < selected.start || selected.end > value.text.length
      || (selected.direction !== 'forward' && selected.direction !== 'backward' && selected.direction !== 'none')) return null;
    selection = { start: selected.start, end: selected.end, direction: selected.direction };
  }
  let attempt: CommentAttempt | null = null;
  if (value.attempt !== null) {
    const tried = value.attempt;
    if (!object(tried) || typeof tried.text !== 'string' || typeof tried.clientId !== 'string' || tried.clientId === '') return null;
    const content = runsOf(tried.content);
    if (content === null) return null;
    const expectedSeq = tried.expectedSeq ?? null;
    if (expectedSeq !== null && (typeof expectedSeq !== 'number' || !Number.isSafeInteger(expectedSeq) || expectedSeq < 0)) return null;
    attempt = { text: tried.text, content, clientId: tried.clientId, expectedSeq };
  }
  return { target, text: value.text, mentions, selection, attempt, revision: value.revision, baseSeq, conflictSeq };
}
const empty = (user: string, owner: string, document: string): CommentDraftData => ({ version: 1, user, owner, document, at: Date.now(), compose: null, focused: null, slots: {} });
const hasData = (data: CommentDraftData): boolean => data.compose !== null || Object.keys(data.slots).length > 0;

function read(user: string, owner: string, document: string, now: number): CommentDraftData | null {
  const key = commentDraftKey(user, owner, document);
  try {
    const raw = localStorage.getItem(key);
    if (raw === null) return null;
    const data: unknown = JSON.parse(raw);
    if (!object(data) || data.version !== 1 || data.user !== user || data.owner !== owner || data.document !== document
      || typeof data.at !== 'number' || !Number.isFinite(data.at) || now - data.at > UNSAVED_KEPT_MS || !object(data.slots)
      || (data.focused !== null && typeof data.focused !== 'string')) { localStorage.removeItem(key); return null; }
    const composed = data.compose;
    let compose: CommentDraftData['compose'] = null;
    if (composed !== null) {
      if (!object(composed) || typeof composed.block !== 'string' || typeof composed.quote !== 'string') { localStorage.removeItem(key); return null; }
      compose = { block: composed.block, quote: composed.quote };
    }
    const slots: Record<string, LocalComment> = {};
    for (const [id, value] of Object.entries(data.slots)) {
      const slot = slotOf(value);
      if (slot === null || id !== commentSlotKey(slot.target)) { localStorage.removeItem(key); return null; }
      slots[id] = slot;
    }
    return { version: 1, user, owner, document, at: data.at, compose, focused: data.focused, slots };
  } catch { return null; }
}

export function getCommentDrafts(user: string, owner: string, document: string): CommentDrafts {
  const key = commentDraftKey(user, owner, document);
  if (user !== client.session.getState().userId || client.session.getState().status !== 'signed_in') {
    return { key, generation: 0, store: createStore<DraftState>(() => ({ data: empty(user, owner, document), saved: true, error: null, pending: new Map(), forget: new Set(), reset: 0 })) };
  }
  const found = controllers.get(key);
  if (found !== undefined) {
    const data = found.store.getState().data;
    if (Date.now() - data.at > UNSAVED_KEPT_MS) {
      try { localStorage.removeItem(key); } catch { /* An expired record is never restored even if deletion is refused. */ }
      found.generation += 1;
      found.store.setState((state) => ({ data: empty(user, owner, document), saved: true, error: null, pending: new Map(), forget: new Set(), reset: state.reset + 1 }));
    }
    return found;
  }
  const retired = retiredFailures.get(key);
  retiredFailures.delete(key);
  const recover = retired !== undefined && Date.now() - retired.data.at <= UNSAVED_KEPT_MS ? retired : undefined;
  const controller: CommentDrafts = { key, generation: 0, store: createStore<DraftState>(() => ({
    data: recover?.data ?? read(user, owner, document, Date.now()) ?? empty(user, owner, document), saved: recover === undefined, error: recover?.error ?? null, pending: new Map(), forget: new Set(recover?.forget), reset: 0,
  })) };
  controllers.set(key, controller);
  controller.store.subscribe(() => updateLossMarks());
  return controller;
}

function retain(controller: CommentDrafts, data: CommentDraftData): boolean {
  const forget = controller.store.getState().forget;
  let wanted = data;
  if (forget.size > 0) {
    const slots = Object.fromEntries(Object.entries(data.slots).filter(([id]) => !forget.has(id)));
    let compose = data.compose;
    for (const id of forget) {
      const removed = data.slots[id];
      if (removed?.target.kind === 'new' && compose?.block === removed.target.block && compose.quote === removed.target.quote) compose = null;
    }
    wanted = { ...data, slots, compose };
  }
  try {
    if (hasData(wanted)) localStorage.setItem(controller.key, JSON.stringify(wanted)); else localStorage.removeItem(controller.key);
    controller.store.setState({ data: wanted, saved: true, error: null, forget: new Set() });
    return true;
  } catch (error) { controller.store.setState({ data, saved: false, error }); return false; }
}
function change(controller: CommentDrafts, update: (before: CommentDraftData) => CommentDraftData): boolean {
  const before = controller.store.getState().data;
  if (controllers.get(controller.key) !== controller || before.user !== client.session.getState().userId || client.session.getState().status !== 'signed_in') return false;
  return retain(controller, { ...update(before), at: Date.now() });
}
export function saveCommentDrafts(controller: CommentDrafts): boolean {
  return change(controller, (data) => data);
}
export function commentIntent(controller: CommentDrafts, compose: CommentDraftData['compose'], focused: string | null): void {
  const before = controller.store.getState().data;
  if (before.compose?.block === compose?.block && before.compose?.quote === compose?.quote && before.focused === focused) return;
  change(controller, (data) => ({ ...data, compose, focused }));
}
export function setCommentText(controller: CommentDrafts, target: CommentTarget, text: string, mentions: readonly CommentMention[] = [], authoredSeq?: number): void {
  const id = commentSlotKey(target);
  change(controller, (data) => {
    if (controller.store.getState().forget.has(id)) {
      const forget = new Set(controller.store.getState().forget); forget.delete(id); controller.store.setState({ forget });
    }
    const old = data.slots[id];
    if (text === '' && target.kind !== 'edit' && (old?.attempt === null || old?.attempt === undefined)) {
      const slots = Object.fromEntries(Object.entries(data.slots).filter(([key]) => key !== id));
      return { ...data, slots };
    }
    const selection = old?.selection === null || old?.selection === undefined ? null : {
      ...old.selection, start: Math.min(old.selection.start, text.length), end: Math.min(old.selection.end, text.length),
    };
    const slot: LocalComment = { target, text, mentions: [...mentions], selection,
      revision: (old?.revision ?? 0) + 1, attempt: old?.attempt ?? null,
      baseSeq: old === undefined ? target.kind === 'edit' && authoredSeq !== undefined && Number.isSafeInteger(authoredSeq) && authoredSeq >= 0 ? authoredSeq : null : old.baseSeq,
      conflictSeq: old?.conflictSeq ?? null };
    return { ...data, slots: { ...data.slots, [id]: slot } };
  });
}
export function selectCommentText(controller: CommentDrafts, target: CommentTarget, selection: CommentSelection): void {
  const id = commentSlotKey(target), old = controller.store.getState().data.slots[id];
  if (old === undefined || selection.start < 0 || selection.end > old.text.length) return;
  if (JSON.stringify(old.selection) === JSON.stringify(selection)) return;
  change(controller, (data) => ({ ...data, slots: { ...data.slots, [id]: { ...old, selection } } }));
}
export function discardComment(controller: CommentDrafts, target: CommentTarget): boolean {
  const id = commentSlotKey(target);
  const before = controller.store.getState().data;
  const slots = Object.fromEntries(Object.entries(before.slots).filter(([key]) => key !== id));
  const next = { ...before, slots, at: Date.now(), compose: target.kind === 'new' && before.compose?.block === target.block && before.compose.quote === target.quote ? null : before.compose };
  if (controllers.get(controller.key) !== controller || before.user !== client.session.getState().userId) return false;
  const forget = new Set(controller.store.getState().forget); forget.add(id); controller.store.setState({ forget });
  if (retain(controller, next)) return true;
  // A refused local erase must leave the affected words reachable in this view.
  controller.store.setState({ data: before });
  return false;
}
export function beginCommentSend(controller: CommentDrafts, target: CommentTarget, text: string, content: CommentRun[]): SendingComment | null {
  const reader = commentReader(), id = commentSlotKey(target), state = controller.store.getState();
  if (!currentCommentReader(reader) || text.trim() === '' || controllers.get(controller.key) !== controller || state.data.user !== reader.user || state.pending.has(id) || state.forget.has(id)) return null;
  const old = state.data.slots[id];
  const expectedSeq = target.kind === 'edit' ? old?.baseSeq ?? null : null;
  if (target.kind === 'edit' && (expectedSeq === null || old?.conflictSeq !== null && old?.conflictSeq !== undefined)) return null;
  const attempt = old?.attempt?.expectedSeq === expectedSeq && JSON.stringify(old.attempt.content) === JSON.stringify(content)
    ? old.attempt : { text: text.trim(), content, clientId: newActKey(), expectedSeq };
  const revision = old?.revision ?? 0;
  // Retain the exact request before explicit Send; a failed local write stays guarded.
  change(controller, (data) => ({ ...data, slots: { ...data.slots, [id]: {
    target, text, mentions: old?.mentions ?? [], selection: old?.selection ?? null, revision, attempt, baseSeq: expectedSeq, conflictSeq: old?.conflictSeq ?? null,
  } } }));
  const request = newActKey(), pending = new Map(state.pending); pending.set(id, request);
  controller.store.setState({ pending });
  return { reader, generation: controller.generation, request, slot: id, revision, attempt };
}

/** An explicit local review chooses exactly the displayed revision; it never sends or adopts a later row. */
export function reviewCommentBase(controller: CommentDrafts, target: Extract<CommentTarget, { kind: 'edit' }>, displayedSeq: number): boolean {
  if (!Number.isSafeInteger(displayedSeq) || displayedSeq < 0) return false;
  const id = commentSlotKey(target), state = controller.store.getState();
  if (state.pending.has(id) || state.forget.has(id) || state.data.slots[id] === undefined) return false;
  return change(controller, (data) => {
    const slot = data.slots[id];
    if (slot === undefined) return data;
    return { ...data, slots: { ...data.slots, [id]: { ...slot, baseSeq: displayedSeq, conflictSeq: null, attempt: null, revision: slot.revision + 1 } } };
  });
}

/** Keep only the conflict revision, never a stale rich response. A receipt-expired retry stays blocked across reload. */
export function markCommentConflict(controller: CommentDrafts, sent: SendingComment, currentSeq: number): void {
  if (!currentCommentReader(sent.reader) || controller.generation !== sent.generation || !Number.isSafeInteger(currentSeq) || currentSeq < 0) return;
  const slot = controller.store.getState().data.slots[sent.slot];
  if (slot?.target.kind !== 'edit' || slot.attempt?.clientId !== sent.attempt.clientId || slot.baseSeq !== sent.attempt.expectedSeq) return;
  change(controller, (data) => ({ ...data, slots: { ...data.slots, [sent.slot]: { ...slot, conflictSeq: currentSeq } } }));
}
export function finishCommentSend(controller: CommentDrafts, sent: SendingComment, success: boolean): boolean {
  if (!currentCommentReader(sent.reader) || controller.generation !== sent.generation) return false;
  const state = controller.store.getState();
  if (state.pending.get(sent.slot) !== sent.request) return false;
  const pending = new Map(state.pending); pending.delete(sent.slot); controller.store.setState({ pending });
  const current = controller.store.getState().data.slots[sent.slot];
  if (!success || current?.revision !== sent.revision || current.attempt?.clientId !== sent.attempt.clientId) return false;
  return discardComment(controller, current.target);
}

function updateLossMarks(): void {
  const next = new Map<string, readonly string[]>();
  const before = lossTargets.getState();
  for (const [view, stores] of bindings) {
    const targets = [...new Set([...stores].filter((controller) => !controller.store.getState().saved).map((controller) => controller.store.getState().data.document))];
    const held = before.get(view);
    if (targets.length > 0) next.set(view, held?.length === targets.length && held.every((id, index) => id === targets[index]) ? held : targets);
    const loss = targets.length > 0;
    if (loss) marked.add(view); else marked.delete(view);
    useWorkbench.getState().setUnsaved(view, loss);
  }
  if (next.size !== before.size || [...next].some(([view, targets]) => before.get(view) !== targets)) lossTargets.setState(next, true);
}
export function useCommentDraftLossTargets(view: string | undefined): readonly string[] {
  return useStore(lossTargets, (state) => view === undefined ? noLossTargets : state.get(view) ?? noLossTargets);
}
export function hasCommentDraftLoss(view: string): boolean { return [...(bindings.get(view) ?? [])].some((controller) => !controller.store.getState().saved); }
/** Stable locators only, for the shared loss workflow to keep the affected target reachable. */
export function commentDraftLossTargets(view: string): readonly string[] {
  return [...new Set([...(bindings.get(view) ?? [])].filter((controller) => !controller.store.getState().saved)
    .map((controller) => controller.store.getState().data.document))];
}
export function commentDraftGuard(view: string): ViewSaveGuard {
  return {
    save: () => {
      let saved = true;
      for (const controller of bindings.get(view) ?? []) {
        const data = controller.store.getState().data;
        if (data.user !== client.session.getState().userId || controllers.get(controller.key) !== controller) { saved = false; continue; }
        if (!retain(controller, { ...data, at: Date.now() })) saved = false;
      }
      return Promise.resolve(saved);
    },
    discard: () => {
      for (const controller of bindings.get(view) ?? []) {
        const before = controller.store.getState().data;
        if (before.user !== client.session.getState().userId || controllers.get(controller.key) !== controller) throw new Error('comment draft reader retired');
        if (!retain(controller, empty(before.user, before.owner, before.document))) {
          controller.store.setState({ data: before });
          throw controller.store.getState().error;
        }
        controller.generation += 1;
        controller.store.setState((state) => ({ reset: state.reset + 1, pending: new Map() }));
      }
    },
  };
}
export function useCommentDraftGuard(view: string | undefined, controller: CommentDrafts): void {
  const guard = useMemo(() => view === undefined ? { save: () => Promise.resolve(false), discard: () => undefined } : commentDraftGuard(view), [view]);
  useViewSaveGuard(view, guard);
  useEffect(() => {
    if (view === undefined) return;
    const held = bindings.get(view) ?? new Set<CommentDrafts>(); held.add(controller); bindings.set(view, held); updateLossMarks();
    return () => {
      if (allTabs(useWorkbench.getState().layout).some((tab) => tab.id === view)) return;
      held.delete(controller); if (held.size === 0) bindings.delete(view);
    };
  }, [view, controller]);
}
export function useCommentDrafts(controller: CommentDrafts): DraftState { return useStore(controller.store); }

/** Device controls call after deleting these keys; a mounted writer must not recreate them. */
export function forgetCommentDraftDeviceData(user: string, keys: readonly string[] | null): void {
  const removed = keys ?? [...new Set([...controllers.keys(), ...retiredFailures.keys()])];
  for (const key of removed) {
    if (key.startsWith(prefix(user))) retiredFailures.delete(key);
    const controller = controllers.get(key);
    if (!key.startsWith(prefix(user)) || controller === undefined) continue;
    const data = controller.store.getState().data;
    controller.generation += 1;
    controller.store.setState((state) => ({ data: empty(data.user, data.owner, data.document), saved: true, error: null, pending: new Map(), forget: new Set(), reset: state.reset + 1 }));
  }
}
/** Retires memory only; each person's seven-day device records remain theirs alone. */
export function retireCommentDraftSession(): void {
  for (const [key, retired] of retiredFailures) if (Date.now() - retired.data.at > UNSAVED_KEPT_MS) retiredFailures.delete(key);
  for (const controller of controllers.values()) {
    const state = controller.store.getState(), data = state.data;
    if (!state.saved && hasData(data) && Date.now() - data.at <= UNSAVED_KEPT_MS) retiredFailures.set(controller.key, { data, error: state.error, forget: new Set(state.forget) });
    controller.generation += 1;
    composeSources.delete(controller);
    controller.store.setState((state) => ({ data: empty(data.user, data.owner, data.document), saved: true, error: null, pending: new Map(), forget: new Set(), reset: state.reset + 1 }));
  }
  controllers.clear(); bindings.clear();
  lossTargets.setState(new Map(), true);
  for (const view of marked) useWorkbench.getState().setUnsaved(view, false);
  marked.clear();
}
client.session.subscribe((next, before) => {
  if (next.userId !== before.userId) retireCommentDraftSession();
  else if (next.readerVersion !== before.readerVersion || next.status !== before.status) {
    for (const controller of controllers.values()) { controller.generation += 1; controller.store.setState({ pending: new Map() }); }
  }
});
useWorkbench.subscribe((next, before) => {
  if (next.layout === before.layout) return;
  const live = new Set(allTabs(next.layout).map((tab) => tab.id));
  for (const view of bindings.keys()) if (!live.has(view)) { bindings.delete(view); marked.delete(view); }
  updateLossMarks();
});
if (typeof window !== 'undefined') window.addEventListener('storage', (event) => {
  if (event.newValue !== null) return;
  if (event.key === null) {
    const people = new Set([...controllers.values()].map((controller) => controller.store.getState().data.user));
    for (const retired of retiredFailures.values()) people.add(retired.data.user);
    for (const user of people) forgetCommentDraftDeviceData(user, null);
  } else {
    const user = controllers.get(event.key)?.store.getState().data.user ?? retiredFailures.get(event.key)?.data.user;
    if (user !== undefined) forgetCommentDraftDeviceData(user, [event.key]);
  }
});
