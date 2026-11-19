/** One visible prompt. Radix retains focus/dismissal; Activity retains covered drafts without running their effects. */
import { Activity, createContext, type ReactNode, useCallback, useContext, useId, useLayoutEffect, useRef, useState } from 'react';
import { create } from 'zustand';

interface Entry { id: string; parent: string | null; kind: 'prompt' | 'reading' | 'scope'; order: number }
interface Prompts { entries: Entry[]; version: number }
const usePrompts = create<Prompts>(() => ({ entries: [], version: 0 }));
const Parent = createContext<string | null>(null);
let nextOpening = 0;

function ancestor(id: string, entry: Entry, entries: readonly Entry[]): boolean {
  const seen = new Set<string>();
  let parent = entry.parent;
  while (parent !== null && !seen.has(parent)) {
    if (parent === id) return true;
    seen.add(parent);
    parent = entries.find((candidate) => candidate.id === parent)?.parent ?? null;
  }
  return false;
}

export function usePrompt(open: boolean, kind: Entry['kind'] = 'prompt', request = 0) {
  const id = useId();
  const parent = useContext(Parent);
  const opening = useRef<number | null>(null);
  const lastRequest = useRef(request);
  useLayoutEffect(() => {
    if (!open) { opening.current = null; return; }
    // Activity reappearance is not a new request: retain the original order above/below other forms.
    if (opening.current === null || lastRequest.current !== request) {
      const enclosing = usePrompts.getState().entries.find((entry) => entry.id === parent);
      // A presenter's loading and loaded dialogs are the same opening, even when their roots differ.
      opening.current = kind === 'prompt' && enclosing?.kind === 'scope' ? enclosing.order : ++nextOpening;
      lastRequest.current = request;
    }
    const order = opening.current;
    usePrompts.setState((state) => {
      const entries = [...state.entries.filter((entry) => entry.id !== id), { id, parent, kind, order }];
      // On Activity reappearance child effects may register first. Equal-order descendants stay on top.
      entries.sort((one, other) => one.order - other.order || (ancestor(one.id, other, entries) ? -1 : ancestor(other.id, one, entries) ? 1 : 0));
      return { entries, version: state.version + 1 };
    });
    return () => usePrompts.setState((state) => ({ entries: state.entries.filter((entry) => entry.id !== id) }));
  }, [id, parent, open, kind, request]);
  const entries = usePrompts((state) => state.entries);
  const top = entries.at(-1);
  const registered = entries.some((entry) => entry.id === id);
  const owner = open ? id : parent;
  const covered = top !== undefined && top.id !== owner;
  const visible = !open || registered && top !== undefined && (top.id === id || ancestor(id, top, entries));
  return { id, owner, visible, covered, style: covered ? { display: 'none' as const } : undefined };
}

export function PromptScope({ layer, children }: { layer: ReturnType<typeof usePrompt>; children: ReactNode }) {
  return <Parent.Provider value={layer.owner}><Activity mode={layer.visible ? 'visible' : 'hidden'}>{children}</Activity></Parent.Provider>;
}

export function usePromptOpen(): boolean { return usePrompts((state) => { const top = state.entries.at(-1); return top !== undefined && top.kind !== 'reading'; }); }

/** Notices wait during a focused reading view as well as during a form. */
export function useNoticesPaused(): boolean { return usePrompts((state) => state.entries.length > 0); }

/** A portal belonging to the current form stays usable; older surfaces stay below its successor. */
export function usePromptCovered(): boolean {
  const owner = useContext(Parent);
  return usePrompts((state) => state.entries.length > 0 && state.entries.at(-1)?.id !== owner);
}

export function onPromptOpened(callback: () => void): () => void {
  return usePrompts.subscribe((next, previous) => { if (next.version !== previous.version) callback(); });
}

/** Keep a card's subtree alive while its child prompt is open, then retire the stale card on return. */
export function useTransientLayer(supplied?: boolean, onChange?: (open: boolean) => void): [boolean, (open: boolean) => void, boolean] {
  const [local, setLocal] = useState(false);
  const covered = usePromptCovered();
  const wasCovered = useRef(false);
  const shown = supplied ?? local;
  const change = useCallback((next: boolean) => { if (supplied === undefined) setLocal(next); onChange?.(next); }, [supplied, onChange]);
  useLayoutEffect(() => {
    if (wasCovered.current && !covered && shown) change(false);
    wasCovered.current = covered;
  }, [covered, shown, change]);
  return [shown, change, covered];
}

export function currentPrompt(): HTMLElement | null {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-prompt-current="true"]')).find((element) =>
    element.isConnected && element.closest('[style*="display: none"], [hidden], [inert], [aria-hidden="true"]') === null && getComputedStyle(element).visibility !== 'hidden') ?? null;
}

export function restorePromptFocus(element: HTMLElement): boolean {
  const current = currentPrompt();
  const sameOwner = current !== null && element.closest('[data-prompt-owner]')?.getAttribute('data-prompt-owner') === current.getAttribute('data-prompt-id');
  return element.isConnected && (current === null || current.contains(element) || sameOwner) && !element.closest('[style*="display: none"], [aria-hidden="true"], [inert]');
}
