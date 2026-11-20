/** Mounted guide entries share one visible destination in a screen; a modal has its own active surface. */
import { useCallback, useId, useSyncExternalStore } from 'react';

/** Keep a named guide first, a group of visible controls next, and an individual hint as the fallback. */
const PREFERENCE = { guide: 2, group: 1, hint: 0 } as const;
type EntryKind = keyof typeof PREFERENCE;
interface Entry { element: HTMLSpanElement; destination: string; kind: EntryKind }
const entries = new Map<string, Entry>();
const listeners = new Set<() => void>();
let suppressed: ReadonlySet<string> = new Set<string>();
let observer: MutationObserver | null = null;
let pending = false;

let ancestors = new WeakMap<HTMLElement, Element | null>();

/** Resolve each shared ancestor once in a refresh. A hidden ancestor covers every entry below it. */
function surface(element: HTMLElement, surfaces: WeakMap<HTMLElement, Element | null>): Element | null {
  if (!element.isConnected) return null;
  const path: HTMLElement[] = [];
  let parent: HTMLElement | null = element;
  while (parent !== null && !surfaces.has(parent)) { path.push(parent); parent = parent.parentElement; }
  let root: Element | null = parent === null ? document.body : surfaces.get(parent) ?? null;
  for (const node of path.reverse()) {
    if (root !== null) {
      if (node.hidden || node.hasAttribute('inert') || node.getAttribute('aria-hidden') === 'true'
        || parent instanceof HTMLDetailsElement && !parent.open && parent.querySelector('summary') !== node) root = null;
      else {
        const style = getComputedStyle(node);
        if (style.display === 'none' || style.visibility === 'hidden') root = null;
        else if (node.matches('[data-prompt-current="true"], [aria-modal="true"], .floating-layer')) root = node;
      }
    }
    surfaces.set(node, root);
    parent = node;
  }
  return root;
}

function refresh(): void {
  const taken = new Map<Element, Set<string>>();
  const next = new Set<string>();
  // Entries share most ancestors. Read each style once in this synchronous refresh,
  // then discard it so the next mutation still observes changed visibility and modals.
  const surfaces = new WeakMap<HTMLElement, Element | null>();
  const ordered = [...entries].sort(([, one], [, other]) => PREFERENCE[other.kind] - PREFERENCE[one.kind]
    || (one.element === other.element ? 0 : (one.element.compareDocumentPosition(other.element) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0 ? -1 : 1));
  for (const [id, entry] of ordered) {
    const root = surface(entry.element, surfaces);
    if (root === null) { next.add(id); continue; }
    const destinations = taken.get(root) ?? new Set<string>();
    if (destinations.has(entry.destination)) next.add(id);
    else destinations.add(entry.destination);
    taken.set(root, destinations);
  }
  ancestors = surfaces;
  if (next.size === suppressed.size && [...next].every((id) => suppressed.has(id))) return;
  suppressed = next;
  for (const listener of listeners) listener();
}

/** All the refs returning with one tab settle before the browser draws their guide buttons. */
function scheduleRefresh(): void {
  if (pending) return;
  pending = true;
  queueMicrotask(() => {
    pending = false;
    refresh();
  });
}

function containsEntry(node: Node): boolean {
  if (!(node instanceof Element)) return false;
  for (const entry of entries.values()) if (node.contains(entry.element)) return true;
  return false;
}

function changesSurface(change: MutationRecord): boolean {
  if (change.type === 'attributes') return change.target instanceof HTMLElement && ancestors.has(change.target);
  // Moving a holder can change its owner, visibility or order. Its own button and text
  // updates elsewhere in the page do not. A closed details element exposes its first summary.
  if (change.target instanceof HTMLDetailsElement && containsEntry(change.target)) return true;
  for (const node of change.addedNodes) if (containsEntry(node)) return true;
  for (const node of change.removedNodes) if (containsEntry(node)) return true;
  return false;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** Prefer the named guide, then a group, then the first hint; restore the next entry when one leaves or hides. */
export function useGuideEntry(article: string | null, heading: string | null, kind: EntryKind = 'hint') {
  const id = useId();
  const ref = useCallback((element: HTMLSpanElement | null) => {
    if (element === null || article === null) {
      if (!entries.delete(id)) return;
    } else entries.set(id, { element, destination: `${article}#${heading ?? ''}`, kind });
    if (entries.size > 0 && observer === null) {
      observer = new MutationObserver((changes) => {
        if (changes.some(changesSurface)) scheduleRefresh();
      });
      observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['hidden', 'inert', 'aria-hidden', 'aria-modal', 'data-prompt-current', 'class', 'style', 'open', 'role'] });
      window.addEventListener('resize', scheduleRefresh);
    } else if (entries.size === 0) {
      observer?.disconnect(); observer = null;
      window.removeEventListener('resize', scheduleRefresh);
    }
    scheduleRefresh();
  }, [id, article, heading, kind]);
  const subscribeEntry = useCallback((listener: () => void) => article === null ? () => undefined : subscribe(listener), [article]);
  const hidden = useSyncExternalStore(subscribeEntry, () => article !== null && suppressed.has(id), () => false);
  return { ref, hidden };
}
