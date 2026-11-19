/** Personal reading choices override installation defaults without adding a read to a hidden view. */
import { QueryClientContext, type QueryCache, type QueryClient } from '@tanstack/react-query';
import { useContext, useSyncExternalStore } from 'react';
import type { Meta, PersonalStateKey } from '@/generated/api-types';

/** Reading choices have their own declared native personal-state key. */
export const READING_STATE_KEY: PersonalStateKey = 'interface';

// Used while the shell's metadata is still arriving; the registry supplies the same defaults.
export const DEFAULT_INTERFACE_READING = { kept_closed_tabs: 10, progress_delay_ms: 200 };
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const count = (value: unknown, fallback: number) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : fallback;

export function interfaceReading(state: Record<string, unknown> | undefined, meta?: Meta) {
  const defaults = meta?.interface_defaults ?? DEFAULT_INTERFACE_READING;
  const own = record(state?.interface) ? state.interface : {};
  return { kept_closed_tabs: count(own.kept_closed_tabs, defaults.kept_closed_tabs), progress_delay_ms: count(own.progress_delay_ms, defaults.progress_delay_ms) };
}

const fallback = { ...DEFAULT_INTERFACE_READING, ready: false };
const fallbackSnapshot = () => fallback;
const skipSubscribe = () => () => undefined;

function makeReadingStore(owner: QueryClient) {
  const cache = owner.getQueryCache();
  const metaHash = owner.defaultQueryOptions({ queryKey: ['meta'] }).queryHash;
  const stateHash = owner.defaultQueryOptions({ queryKey: ['me-state'] }).queryHash;
  const listeners = new Set<() => void>();
  let snapshot = fallback;
  let freshness: Record<string, unknown> = {};
  const update = () => {
    const meta = cache.get<Meta>(metaHash)?.state;
    const state = cache.get<Record<string, unknown>>(stateHash)?.state;
    const personal = state?.data?.interface;
    const windows = record(personal) ? personal.read_freshness_ms : undefined;
    const defaults = meta?.data?.interface_defaults;
    freshness = { ...defaults?.read_freshness_ms, ...(record(windows) ? windows : {}) };
    const next = interfaceReading(state?.data, meta?.data);
    const ready = meta?.status === 'success' && state?.status === 'success';
    if (snapshot.ready === ready && snapshot.kept_closed_tabs === next.kept_closed_tabs && snapshot.progress_delay_ms === next.progress_delay_ms) return;
    snapshot = { ...next, ready };
    for (const notify of listeners) notify();
  };
  update();
  // The two cached answers supply every control. Other queries and observer changes carry no preferences.
  cache.subscribe((event) => {
    if ((event.query.queryHash === metaHash || event.query.queryHash === stateHash)
      && (event.type === 'added' || event.type === 'removed' || event.type === 'updated')) update();
  });
  return {
    subscribe: (notify: () => void) => {
      listeners.add(notify);
      return () => { listeners.delete(notify); };
    },
    snapshot: () => snapshot,
    freshness: () => freshness,
  };
}

const stores = new WeakMap<QueryCache, ReturnType<typeof makeReadingStore>>();
export function readingPreferences(client: QueryClient) {
  const cache = client.getQueryCache();
  let store = stores.get(cache);
  if (store === undefined) {
    store = makeReadingStore(client);
    stores.set(cache, store);
  }
  return store;
}

export function useInterfaceReading(enabled = true) {
  const client = useContext(QueryClientContext);
  const store = enabled && client !== undefined ? readingPreferences(client) : undefined;
  return useSyncExternalStore(store?.subscribe ?? skipSubscribe, store?.snapshot ?? fallbackSnapshot, fallbackSnapshot);
}
