/** A person's view choices on this device, shared by layouts, routes, disclosures and windows. */
import { useCallback, useEffect, useMemo, useSyncExternalStore } from 'react';
import { useStore } from 'zustand';

import { client } from '@/api/client';

export type ViewChoice = string | number | boolean | null;
const listeners = new Set<() => void>();
const changed = () => { for (const listener of listeners) listener(); };
const keyOf = (user: string, kind: string, key: string, thing?: string) => `orch.${user}.choices.${kind}.${thing === undefined ? '' : `${thing}.`}${key}`;
const memory = new Map<string, ViewChoice>();
const generations = new Map<string, number>();
const observed = new Map<string, number>();
// Enough for 1,000 small page states when browser storage is unavailable; old fallback pages may be revisited from defaults.
const FALLBACK_BYTES = 4 * 1024 * 1024;
const memoryBytes = new Map<string, number>();
let heldBytes = 0;
function forgetMemory(key: string) {
  heldBytes -= memoryBytes.get(key) ?? 0;
  memoryBytes.delete(key);
  memory.delete(key);
}
function rememberMemory(key: string, value: ViewChoice) {
  forgetMemory(key);
  const bytes = 2 * (key.length + JSON.stringify(value).length);
  if (bytes > FALLBACK_BYTES) return;
  while (heldBytes + bytes > FALLBACK_BYTES) {
    const oldest = memory.keys().next().value;
    if (oldest === undefined) break;
    forgetMemory(oldest);
  }
  memory.set(key, value);
  memoryBytes.set(key, bytes);
  heldBytes += bytes;
}

/** The meaning of a saved choice, for the person's device-data controls. */
export function viewChoiceKeyDetails(user: string, storedKey: string): { kind: string; key: string } | null {
  const prefix = `orch.${user}.choices.`;
  if (!storedKey.startsWith(prefix)) return null;
  const [kind, ...parts] = storedKey.slice(prefix.length).split('.');
  const key = parts.at(-1);
  return kind === undefined || kind === '' || key === undefined || key === '' ? null : { kind, key };
}

// A new window reads the latest stored choice. A window already in use moves only for its own acts.

/** Reads the thing's deliberate override, then its kind's choice, without storing a default. */
export function readViewChoice<T extends ViewChoice>(user: string | null, kind: string, key: string, fallback: T, thing?: string): T {
  if (user === null) return fallback;
  const read = (storedKey: string): ViewChoice | undefined => {
    if (memory.has(storedKey)) return memory.get(storedKey);
    try {
      const raw = localStorage.getItem(storedKey);
      if (raw === null) return undefined;
      const value: unknown = JSON.parse(raw);
      if (value === null || ['string', 'number', 'boolean'].includes(typeof value)) return value as ViewChoice;
      return undefined;
    } catch { return memory.get(storedKey); }
  };
  const own = thing === undefined ? undefined : read(keyOf(user, kind, key, thing));
  const value = own === undefined ? read(keyOf(user, kind, key)) : own;
  return value === undefined || (fallback !== null && value !== null && typeof value !== typeof fallback) ? fallback : value as T;
}

/** Keeps only the choice a person made; unavailable choices are never overwritten by a fallback. */
export function writeViewChoice(user: string | null, kind: string, key: string, value: ViewChoice, thing?: string): void {
  if (user === null) return;
  const storedKey = keyOf(user, kind, key, thing);
  if (observed.has(storedKey)) generations.set(storedKey, (generations.get(storedKey) ?? 0) + 1);
  try { localStorage.setItem(storedKey, JSON.stringify(value)); forgetMemory(storedKey); } catch { rememberMemory(storedKey, value); }
  changed();
}

/** Device-data controls clear the same reader's transient fallback along with successfully removed stored keys. */
export function forgetViewChoiceDeviceData(user: string, keys: readonly string[] | null): void {
  const prefix = `orch.${user}.choices.`;
  const selected = keys === null ? null : new Set(keys);
  const matches = (key: string) => key.startsWith(prefix) && (selected === null || selected.has(key));
  for (const key of memory.keys()) if (matches(key)) forgetMemory(key);
  for (const key of observed.keys()) if (matches(key)) generations.set(key, (generations.get(key) ?? 0) + 1);
  changed();
}

function subscribeChoice(key: string, listener: () => void) {
  observed.set(key, (observed.get(key) ?? 0) + 1);
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
    const remaining = (observed.get(key) ?? 1) - 1;
    if (remaining === 0) {
      observed.delete(key);
      generations.delete(key);
    } else observed.set(key, remaining);
  };
}

/** Reads a saved choice when a view opens, retaining the live choice of a window already in use. */
export function useViewChoice<T extends ViewChoice>(kind: string, key: string, fallback: T, thing?: string): [T, (value: T) => void] {
  const user = useStore(client.session, (state) => state.userId);
  const storedKey = keyOf(user ?? '', kind, key, thing);
  const subscribe = useCallback((listener: () => void) => subscribeChoice(storedKey, listener), [storedKey]);
  const generation = useSyncExternalStore(subscribe, () => generations.get(storedKey) ?? 0, () => 0);
  // Local writes invalidate this snapshot. External writes initialize a new view without moving one in use.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const choice = useMemo(() => readViewChoice(user, kind, key, fallback, thing), [user, kind, key, fallback, thing, generation]);
  const choose = useCallback((value: T) => writeViewChoice(user, kind, key, value, thing), [user, kind, key, thing]);
  return [choice, choose];
}

/** An explicit address wins; a plain address resumes the last valid view of this kind. */
export function useRememberedView<T extends string>(kind: string, key: string, explicit: T | undefined, fallback: T, offered: readonly T[]): [T, (value: T) => void] {
  const [kept, choose] = useViewChoice<string>(kind, key, fallback);
  useEffect(() => { if (explicit !== undefined) choose(explicit); }, [explicit, choose]);
  const wanted = explicit ?? kept;
  return [offered.includes(wanted as T) ? wanted as T : fallback, choose];
}
