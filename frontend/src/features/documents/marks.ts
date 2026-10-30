/** What the page marks beside its text: the blocks that changed since the reader looked, the block a link led to, and the words a discussion is about. */
import { create, useStore, type StoreApi } from 'zustand';
import { createContext, createElement, useContext, useMemo, type ReactNode } from 'react';
import type { Editor } from '@tiptap/core';
import { client } from '@/api/client';

/** A discussion's place on the page. */
export interface Anchor {
  discussion: string;
  block: string | null;
  quote: string;
  open: boolean;
}

export interface Marks {
  /** The blocks changed since the reader last had the page open, with who changed each. */
  changed: ReadonlyMap<string, string>;
  /** The block a link, a hit or a discussion led to; marked for a moment. */
  highlight: string | null;
  anchors: readonly Anchor[];
  /** The discussion whose words were pressed. */
  focused: string | null;
  /** A discussion the person is about to open, on a block and on some words of it. */
  compose: { block: string; quote: string } | null;
}

const initial = (): Marks => ({ changed: new Map(), highlight: null, anchors: [], focused: null, compose: null });
export const useMarks = create<Marks>(initial);
const MarksContext = createContext<StoreApi<Marks>>(useMarks);
const ScopeContext = createContext<{ owner: string; documentId: string } | null>(null);
const stores = new Map<string, StoreApi<Marks>>();
const editors = new WeakMap<Editor, StoreApi<Marks>>();
const ownedEditors = new WeakMap<StoreApi<Marks>, Set<Editor>>();
/** The same per-view store used by its provider, including before an addressed page mounts. */
export function documentMarksFor(owner: string, documentId: string): StoreApi<Marks> {
  const key = JSON.stringify([owner, documentId]);
  let store = stores.get(key);
  if (store === undefined) { store = create<Marks>(initial); stores.set(key, store); }
  return store;
}
export function DocumentMarksProvider({ owner, documentId, children }: { owner: string; documentId: string; children: ReactNode }): ReactNode {
  // A reused tab ID cannot carry another signed-in reader's selection/compose map.
  useStore(client.session, (state) => state.userId);
  useStore(client.session, (state) => state.readerVersion);
  const store = documentMarksFor(owner, documentId);
  const scope = useMemo(() => ({ owner, documentId }), [owner, documentId]);
  return createElement(ScopeContext.Provider, { value: scope }, createElement(MarksContext.Provider, { value: store }, children));
}
/** Fixing a currently displayed Comments panel keeps its compose/selection state with that panel. */
export function retainDocumentMarks(from: string, to: string, documentId: string): void {
  const held = stores.get(JSON.stringify([from, documentId]));
  const key = JSON.stringify([to, documentId]);
  if (held !== undefined && !stores.has(key)) stores.set(key, create<Marks>(() => held.getState()));
}
export function forgetDocumentMarks(owner: string): void {
  for (const key of stores.keys()) if ((JSON.parse(key) as string[])[0] === owner) stores.delete(key);
}
export function useDocumentMarksStore(): StoreApi<Marks> { return useContext(MarksContext); }
export function useDocumentMarksScope(): { owner: string; documentId: string } | null { return useContext(ScopeContext); }
export function useDocumentMarks<Value>(select: (state: Marks) => Value): Value { return useStore(useDocumentMarksStore(), select); }
export function bindEditorMarks(editor: Editor, marks: StoreApi<Marks>): () => void {
  editors.set(editor, marks);
  const owned = ownedEditors.get(marks) ?? new Set<Editor>(); owned.add(editor); ownedEditors.set(marks, owned);
  return () => { owned.delete(editor); if (editors.get(editor) === marks) editors.delete(editor); };
}
export function marksOf(editor: Editor): StoreApi<Marks> { return editors.get(editor) ?? useMarks; }

/** A comment anchor scrolls only the editor sharing this pane's marks, including duplicate documents. */
export function scrollToDocumentBlock(id: string, marks: StoreApi<Marks>): void {
  for (const editor of ownedEditors.get(marks) ?? []) {
    if (editor.isDestroyed || !editor.view.dom.isConnected) continue;
    editor.view.dom.querySelector(`[data-id="${CSS.escape(id)}"]`)?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  }
}

/** Reveal only an editor bound to this pane; folding and the highlight remain local presentation. */
export function revealDocumentBlock(id: string, marks: StoreApi<Marks>, current: () => boolean = () => true, only?: Editor): boolean {
  if (!current()) return false;
  const visible = (root: HTMLElement) => {
    if (!root.isConnected || root.closest('[hidden], [inert], [aria-hidden="true"]') !== null) return false;
    for (let element: HTMLElement | null = root; element !== null; element = element.parentElement) {
      const style = getComputedStyle(element);
      if (style.display === 'none' || style.visibility === 'hidden') return false;
    }
    return true;
  };
  for (const editor of ownedEditors.get(marks) ?? []) {
    if (only !== undefined && editor !== only) continue;
    if (editor.isDestroyed) continue;
    const root = editor.view.dom;
    if (!visible(root)) continue;
    const target = root.querySelector<HTMLElement>(`[data-id="${CSS.escape(id)}"]`);
    if (target === null) continue;
    for (let parent = target.parentElement?.closest<HTMLElement>('[data-id]'); parent !== null && parent !== undefined && root.contains(parent); parent = parent.parentElement?.closest<HTMLElement>('[data-id]')) {
      parent.querySelector<HTMLButtonElement>(':scope > [data-folded="true"] > .oc-fold')?.click();
    }
    // Local folds render before scrolling. A retired/replaced owner cannot publish a late highlight.
    requestAnimationFrame(() => {
      if (!current() || ownedEditors.get(marks)?.has(editor) !== true || editor.isDestroyed || !visible(root) || !root.contains(target)) return;
      target.scrollIntoView({ block: 'center', behavior: 'smooth' }); highlightBlock(id, marks);
    });
    return true;
  }
  return false;
}

/** Retire both registry entries and already-held stores; old mounted references see no old intent. */
export function retireDocumentMarks(): void {
  const retired = [...stores.values()];
  stores.clear();
  // Keep old editors bound to their retired stores until their normal cleanup.
  // Clearing the WeakMap would send an old editor to the next reader's fallback.
  for (const store of retired) { ownedEditors.delete(store); store.setState(initial()); }
  ownedEditors.delete(useMarks);
  useMarks.setState(initial());
}
client.session.subscribe((next, before) => {
  if (next.userId !== before.userId || next.readerVersion !== before.readerVersion) retireDocumentMarks();
});


/** How long a block that was led to stays marked. */
export const HIGHLIGHT_MS = 2_000;

/** Marks a block for a moment. */
export function highlightBlock(id: string, marks: StoreApi<Marks> = useMarks): void {
  marks.setState({ highlight: id });
  setTimeout(() => {
    if (marks.getState().highlight === id) marks.setState({ highlight: null });
  }, HIGHLIGHT_MS);
}
