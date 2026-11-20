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

const ancestors = new WeakSet<HTMLElement>();
const changedDestinations = new Set<string>();

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
    ancestors.add(node);
    parent = node;
  }
  return root;
}

function refresh(): void {
  const taken = new Map<Element, Set<string>>();
  const next = new Set([...suppressed].filter((id) => {
    const entry = entries.get(id);
    return entry !== undefined && !changedDestinations.has(entry.destination);
  }));
  // Entries share most ancestors. Read each style once in this synchronous refresh,
  // then discard it so the next mutation still observes changed visibility and modals.
  const surfaces = new WeakMap<HTMLElement, Element | null>();
  const ordered = [...entries].filter(([, entry]) => changedDestinations.has(entry.destination)).sort(([, one], [, other]) => PREFERENCE[other.kind] - PREFERENCE[one.kind]
    || (one.element === other.element ? 0 : (one.element.compareDocumentPosition(other.element) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0 ? -1 : 1));
  for (const [id, entry] of ordered) {
    const root = surface(entry.element, surfaces);
    if (root === null) { next.add(id); continue; }
    const destinations = taken.get(root) ?? new Set<string>();
    if (destinations.has(entry.destination)) next.add(id);
    else destinations.add(entry.destination);
    taken.set(root, destinations);
  }
  changedDestinations.clear();
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

/** A guide can only displace another entry for the same article and heading. */
function markSubtree(node: Node): void {
  if (!(node instanceof Element)) return;
  for (const entry of entries.values()) {
    if (!changedDestinations.has(entry.destination) && node.contains(entry.element)) changedDestinations.add(entry.destination);
  }
}

function surfaceChanges(changes: MutationRecord[]): void {
  for (const change of changes) {
    if (change.type === 'attributes') {
      if (change.target instanceof HTMLElement && ancestors.has(change.target)) markSubtree(change.target);
    } else {
      // Moving a holder can change its owner, visibility or order. A closed details
      // element also changes which children are visible when its first summary moves.
      if (change.target instanceof HTMLDetailsElement) markSubtree(change.target);
      for (const node of change.addedNodes) markSubtree(node);
      for (const node of change.removedNodes) markSubtree(node);
    }
  }
  if (changedDestinations.size > 0) scheduleRefresh();
}

function resize(): void {
  for (const entry of entries.values()) changedDestinations.add(entry.destination);
  scheduleRefresh();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/** Prefer the named guide, then a group, then the first hint; restore the next entry when one leaves or hides. */
export function useGuideEntry(article: string | null, heading: string | null, kind: EntryKind = 'hint') {
  const id = useId();
  const ref = useCallback((element: HTMLSpanElement | null) => {
    const previous = entries.get(id);
    if (previous !== undefined) changedDestinations.add(previous.destination);
    if (element === null || article === null) {
      if (!entries.delete(id)) return;
    } else {
      const destination = `${article}#${heading ?? ''}`;
      entries.set(id, { element, destination, kind });
      changedDestinations.add(destination);
    }
    if (entries.size > 0 && observer === null) {
      observer = new MutationObserver(surfaceChanges);
      observer.observe(document.body, { childList: true, subtree: true, attributes: true, attributeFilter: ['hidden', 'inert', 'aria-hidden', 'aria-modal', 'data-prompt-current', 'class', 'style', 'open', 'role'] });
      window.addEventListener('resize', resize);
    } else if (entries.size === 0) {
      observer?.disconnect(); observer = null;
      window.removeEventListener('resize', resize);
    }
    scheduleRefresh();
  }, [id, article, heading, kind]);
  const subscribeEntry = useCallback((listener: () => void) => article === null ? () => undefined : subscribe(listener), [article]);
  const hidden = useSyncExternalStore(subscribeEntry, () => article !== null && suppressed.has(id), () => false);
  return { ref, hidden };
}
