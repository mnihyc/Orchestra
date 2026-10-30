/** Visible slots give kept views one logical opening without treating hidden updates or moves as reopenings. */
import { Activity, createContext, type ReactNode, useCallback, useContext, useLayoutEffect, useState, useSyncExternalStore } from 'react';

import { openingId } from '@/lib/opening';
import { newActKey } from '@/lib/requests';

/** Undefined is a page outside the workbench; null is a kept view with no visible slot. */
export const ViewOpeningContext = createContext<string | null | undefined>(undefined);
/** The nonce of the displayed route, held while another route is still loading. */
export const ViewRouteContext = createContext<string | undefined>(undefined);
const ViewWorkContext = createContext<((owner: symbol) => () => void) | null>(null);

/** An unfinished local act keeps its own effects alive while its tab is hidden. */
export function useViewWork(pending: boolean): void {
  const hold = useContext(ViewWorkContext);
  const [owner] = useState(() => Symbol('view work'));
  useLayoutEffect(() => pending ? hold?.(owner) : undefined, [hold, owner, pending]);
}

/** Outside a kept view the caller is on screen; a kept hidden view must disable its data readers. */
export function useViewShown(): boolean {
  return useContext(ViewOpeningContext) !== null;
}

interface Opening {
  slots: Map<symbol, string>;
  visible: boolean;
  route: string;
  nonce: string | null;
}

const openings = new Map<string, Opening>();
const instanceRoutes = new Map<string, string>();
/** A tab ID is local presentation identity; opening receipts require their own valid UUID. */
export function openingForView(id: string): string {
  let nonce = instanceRoutes.get(id);
  if (nonce === undefined) { nonce = newActKey(); instanceRoutes.set(id, nonce); }
  return nonce;
}

const listeners = new Set<() => void>();
const pending = new Set<string>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

function reconcile(key: string): void {
  if (pending.has(key)) return;
  pending.add(key);
  // A section can leave one slot and enter another in the same commit. Only a settled absence closes it.
  queueMicrotask(() => {
    pending.delete(key);
    const entry = openings.get(key);
    if (entry === undefined) return;
    const visible = entry.slots.size > 0;
    const route = Array.from(entry.slots.values()).at(-1);
    if (visible === entry.visible && (!visible || route === entry.route)) return;
    if (visible) {
      if (route === undefined) return;
      entry.nonce = entry.nonce === null || entry.route !== route ? route : newActKey();
      entry.route = route;
    }
    entry.visible = visible;
    for (const listener of listeners) listener();
  });
}

/** Called only by a displayed slot, not by the kept body that can continue reading in the background. */
export function showViewOpening(key: string, route = openingId()): () => void {
  let entry = openings.get(key);
  if (entry === undefined) {
    entry = { slots: new Map(), visible: false, route, nonce: null };
    openings.set(key, entry);
  }
  const slot = Symbol(key);
  entry.slots.set(slot, route);
  reconcile(key);
  const current = entry;
  return () => { current.slots.delete(slot); reconcile(key); };
}

export function forgetViewOpening(key: string): void {
  instanceRoutes.delete(key);
  if (!openings.delete(key)) return;
  for (const listener of listeners) listener();
}

/** A new person must not inherit another reader's already-admitted view lifetime. */
export function clearViewOpenings(): void {
  openings.clear();
  instanceRoutes.clear();
  for (const listener of listeners) listener();
}

function visibleOpening(key: string, route: string): string | null {
  const entry = openings.get(key);
  if (entry?.visible !== true) return null;
  return entry.route === route ? entry.nonce : route;
}

export function ViewOpening({ view, route, children, hold = false }: { view: string; route: string; children: ReactNode; hold?: boolean }) {
  const [work, setWork] = useState<ReadonlySet<symbol>>(new Set());
  const keep = useCallback((owner: symbol) => {
    setWork((before) => new Set([...before, owner]));
    return () => setWork((before) => { const next = new Set(before); next.delete(owner); return next; });
  }, []);
  const nonce = useSyncExternalStore(subscribe, () => visibleOpening(view, route), () => null);
  const started = useSyncExternalStore(subscribe, () => (openings.get(view)?.nonce ?? null) !== null, () => false);
  // Unseen views must not even render: hidden Activity alone can pre-render Suspense data/code.
  if (!started) return null;
  // Later hiding preserves drafts/DOM but tears down query observers, timers and live subscriptions.
  return <ViewOpeningContext.Provider value={nonce}><ViewWorkContext.Provider value={keep}><Activity mode={nonce === null && !hold && work.size === 0 ? 'hidden' : 'visible'}>{children}</Activity></ViewWorkContext.Provider></ViewOpeningContext.Provider>;
}
