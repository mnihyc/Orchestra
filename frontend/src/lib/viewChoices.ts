/** A person's view choices on this device, shared by layouts, routes, disclosures and windows. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { createStore, type StoreApi } from 'zustand/vanilla';
import { useStore } from 'zustand';

import { client } from '@/api/client';
import { requestReader } from '@/lib/readAttempts';
import { bindDeviceChoice, deviceChoiceKind, type DeviceChoiceFailure, type DeviceChoiceKind, type DeviceChoiceObservation, type DeviceChoiceRead, type DeviceChoiceSave } from '@/lib/viewChoiceFacts';

export type ViewChoice = string | number | boolean | null;
const listeners = new Set<() => void>();
const changed = () => { for (const listener of listeners) listener(); };
const keyOf = (user: string, kind: string, key: string, thing?: string) => `orch.${user}.choices.${kind}.${thing === undefined ? '' : `${thing}.`}${key}`;
const memory = new Map<string, ViewChoice>();
const memoryReaders = new Map<string, string>();
interface ChoiceSnapshot { value: ViewChoice; observation?: DeviceChoiceObservation; chosen?: ViewChoice; readKeys?: readonly string[] }
interface ChoiceOwner { user: string | null; reader: string; kind: string; key: string; thing: string | undefined; storedKey: string; fallback: ViewChoice; store: StoreApi<ChoiceSnapshot> }
// Weak routing reaches a kept view even while its effects pause, without retaining departed private values.
const owners = new Set<WeakRef<ChoiceOwner>>();
const retiredOwners = new FinalizationRegistry<WeakRef<ChoiceOwner>>((ref) => { owners.delete(ref); });
function* liveOwners() {
  for (const ref of owners) {
    const owner = ref.deref();
    if (owner === undefined) owners.delete(ref); else yield owner;
  }
}
const NO_FAILURES: readonly DeviceChoiceFailure[] = [];
let failureReader = '', failures = NO_FAILURES;
// A soft budget for about 1,000 unobserved page states; live controls always retain their own refused value.
const FALLBACK_BYTES = 4 * 1024 * 1024;
const memoryBytes = new Map<string, number>();
let heldBytes = 0;
function forgetMemory(key: string) {
  heldBytes -= memoryBytes.get(key) ?? 0;
  memoryBytes.delete(key);
  memory.delete(key);
  memoryReaders.delete(key);
}
function pruneMemory() {
  for (const key of memory.keys()) {
    if (heldBytes <= FALLBACK_BYTES) break;
    forgetMemory(key);
  }
}
function rememberMemory(key: string, value: ViewChoice, reader?: string) {
  forgetMemory(key);
  const bytes = 2 * (key.length + JSON.stringify(value).length);
  memory.set(key, value);
  if (reader !== undefined) memoryReaders.set(key, reader);
  memoryBytes.set(key, bytes);
  heldBytes += bytes;
  pruneMemory();
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
  return readChoice(user, kind, key, fallback, thing).value;
}

/** The value and its receipt come from the same existing storage operation. */
function readChoice<T extends ViewChoice>(user: string | null, kind: string, key: string, fallback: T, thing?: string): { value: T; observation?: DeviceChoiceObservation<T>; readKeys?: readonly string[] } {
  if (user === null) return { value: fallback };
  const read = (storedKey: string): { value?: ViewChoice; saved?: ViewChoice; result: DeviceChoiceRead } => {
    if (memory.has(storedKey)) return { value: memory.get(storedKey), result: 'memory' };
    let raw: string | null;
    try { raw = localStorage.getItem(storedKey); }
    catch { return { value: memory.get(storedKey), result: 'unavailable' }; }
    if (raw === null) return { result: 'available' };
    try {
      const value: unknown = JSON.parse(raw);
      if (value === null || typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean') return { value, saved: value, result: 'available' };
      return { result: 'invalid' };
    } catch { return { result: 'invalid' }; }
  };
  const own = thing === undefined ? undefined : read(keyOf(user, kind, key, thing));
  const chosen = own?.value === undefined ? read(keyOf(user, kind, key)) : own;
  const value = chosen.value;
  const valid = value !== undefined && !(fallback !== null && value !== null && typeof value !== typeof fallback);
  const effective = valid ? value as T : fallback;
  const state = client.session.getState(), reader = requestReader(user, state.readerVersion);
  const result = own?.result === 'unavailable' || chosen.result === 'unavailable' ? 'unavailable' : own?.result === 'invalid' ? 'invalid' : chosen.result;
  const readKeys = thing === undefined ? [keyOf(user, kind, key)] : own?.value === undefined ? [keyOf(user, kind, key, thing), keyOf(user, kind, key)] : [keyOf(user, kind, key, thing)];
  return { value: effective, readKeys, observation: { reader, scope: { kind, key, ...(thing === undefined ? {} : { thing }) }, chosen: value, effective, saved: own?.result === 'invalid' ? undefined : chosen.saved,
    origin: !valid ? 'default' : chosen.result === 'memory' ? 'page' : own?.value !== undefined ? 'item' : kind === 'page' || kind.startsWith('page.') ? 'page' : 'kind',
    saved_choice_availability: result === 'invalid' ? 'unavailable' : value === undefined ? 'unreported' : valid ? 'available' : 'unavailable',
    read_result: result, write_result: chosen.result === 'memory' && memoryReaders.get(own?.value === undefined ? keyOf(user, kind, key) : keyOf(user, kind, key, thing)) === reader ? 'failed' : 'not_checked' } };
}

/** Keeps only the choice a person made; unavailable choices are never overwritten by a fallback. */
export function writeViewChoice(user: string | null, kind: string, key: string, value: ViewChoice, thing?: string): void {
  if (user === null) return;
  const storedKey = keyOf(user, kind, key, thing);
  const current = client.session.getState(), reader = requestReader(user, current.readerVersion);
  const serialized = JSON.stringify(value);
  let result: DeviceChoiceSave;
  try { localStorage.setItem(storedKey, serialized); forgetMemory(storedKey); result = 'saved'; }
  catch { rememberMemory(storedKey, value, current.userId === user ? reader : undefined); result = 'failed'; }
  if (current.userId === user) {
    const applied = result === 'saved' ? JSON.parse(serialized) as ViewChoice : value;
    for (const owner of liveOwners()) {
      if (owner.storedKey !== storedKey || owner.reader !== reader) continue;
      const before = owner.store.getState().observation;
      const valid = applied === null || owner.fallback === null || typeof applied === typeof owner.fallback;
      const effective = valid ? applied : owner.fallback;
      owner.store.setState({ value: effective, readKeys: [storedKey], observation: { reader, scope: { kind, key, ...(thing === undefined ? {} : { thing }) },
        chosen: applied, effective, saved: result === 'saved' ? applied : before?.saved,
        origin: !valid ? 'default' : result === 'failed' ? 'page' : thing !== undefined ? 'item' : kind === 'page' || kind.startsWith('page.') ? 'page' : 'kind',
        saved_choice_availability: result === 'saved' ? valid ? 'available' : 'unavailable' : before?.saved_choice_availability ?? 'unreported',
        read_result: result === 'failed' ? 'memory' : 'available', write_result: result } }, true);
    }
    recordDeviceChoiceSave(user, kind, result);
  }
  changed();
}

/** Other device adapters retain the same safe disappeared-control observation from their actual write. */
export function recordDeviceChoiceSave(user: string, kind: string, result: DeviceChoiceSave): void {
  const current = client.session.getState();
  if (current.userId !== user || result !== 'failed') return;
  const reader = requestReader(user, current.readerVersion);
  if (failureReader !== reader) { failureReader = reader; failures = NO_FAILURES; }
  const kindName = deviceChoiceKind(kind);
  if (!failures.some((failure) => failure.kind === kindName)) { failures = [...failures, { kind: kindName, write_result: 'failed' }]; changed(); }
}

/** A disappeared control retains only a safe kind and observed refusal, never its private value. */
export function useDeviceChoiceFailures(): readonly DeviceChoiceFailure[] {
  const reader = useStore(client.session, (state) => requestReader(state.userId, state.readerVersion));
  return useSyncExternalStore((listener) => { listeners.add(listener); return () => { listeners.delete(listener); }; },
    () => failureReader === reader ? failures : NO_FAILURES, () => NO_FAILURES);
}

/** Called after this kind's stored records were removed successfully, or for its volatile-only choice. */
export function forgetViewChoiceKind(user: string, kind: DeviceChoiceKind): void {
  const keys = [...new Set([...memory.keys(), ...Array.from(liveOwners(), (owner) => owner.storedKey)])].filter((key) => {
    const detail = viewChoiceKeyDetails(user, key);
    return detail !== null && deviceChoiceKind(detail.kind) === kind;
  });
  forgetViewChoiceDeviceData(user, keys);
  const current = client.session.getState();
  if (current.userId === user && failureReader === requestReader(user, current.readerVersion)) failures = failures.filter((failure) => failure.kind !== kind);
  changed();
}

/** Device-data controls clear the same reader's transient fallback along with successfully removed stored keys. */
export function forgetViewChoiceDeviceData(user: string, keys: readonly string[] | null): void {
  const prefix = `orch.${user}.choices.`;
  const selected = keys === null ? null : new Set(keys);
  const matches = (key: string) => key.startsWith(prefix) && (selected === null || selected.has(key));
  for (const key of memory.keys()) if (matches(key)) forgetMemory(key);
  for (const owner of liveOwners()) {
    if (owner.user !== user || !matches(owner.storedKey) && !owner.store.getState().readKeys?.some(matches)) continue;
    const next = readChoice(user, owner.kind, owner.key, owner.fallback, owner.thing);
    owner.store.setState({ ...next, ...(next.observation === undefined ? {} : { observation: { ...next.observation, reader: owner.reader } }) }, true);
  }
  if (keys === null && client.session.getState().userId === user) { failureReader = ''; failures = NO_FAILURES; }
  changed();
}

function createChoiceOwner(user: string | null, reader: string, kind: string, key: string, fallback: ViewChoice, thing: string | undefined, before?: ChoiceOwner): ChoiceOwner {
  let initial: ChoiceSnapshot;
  if (before === undefined) initial = readChoice(user, kind, key, fallback, thing);
  else {
    const prior = before.store.getState(), fact = prior.observation;
    if (fact === undefined) initial = prior.chosen === undefined ? { value: fallback } : { value: prior.chosen, chosen: prior.chosen };
    else {
      const chosen = fact.chosen;
      const valid = chosen !== undefined && (chosen === null || typeof chosen === 'string' || typeof chosen === 'number' || typeof chosen === 'boolean')
        && !(fallback !== null && chosen !== null && typeof chosen !== typeof fallback);
      const value = valid ? chosen : fallback;
      initial = { value, readKeys: prior.readKeys, observation: { ...fact, effective: value, origin: valid ? fact.read_result === 'memory' ? 'page' : fact.origin : 'default' } };
    }
  }
  const owner: ChoiceOwner = { user, reader, kind, key, thing, storedKey: keyOf(user ?? '', kind, key, thing), fallback, store: createStore<ChoiceSnapshot>(() => initial) };
  const ref = new WeakRef(owner); owners.add(ref); retiredOwners.register(owner, ref);
  return owner;
}

/** The view owns its value through hiding; the unobserved fallback cache never owns live state. */
export function useViewChoice<T extends ViewChoice>(kind: string, key: string, fallback: T, thing?: string): [T, (value: T) => void, DeviceChoiceObservation<T>?] {
  const user = useStore(client.session, (state) => state.userId);
  const version = useStore(client.session, (state) => state.readerVersion);
  const reader = requestReader(user, version), storedKey = keyOf(user ?? '', kind, key, thing);
  const [held, setHeld] = useState(() => createChoiceOwner(user, reader, kind, key, fallback, thing));
  let owner = held;
  if (held.storedKey !== storedKey || held.reader !== reader || !Object.is(held.fallback, fallback)) {
    owner = createChoiceOwner(user, reader, kind, key, fallback, thing, held.storedKey === storedKey && held.reader === reader ? held : undefined);
    setHeld(owner);
  }
  const choice = useStore(owner.store);
  const choose = useCallback((value: T) => {
    const current = client.session.getState();
    if (current.userId !== owner.user || requestReader(current.userId, current.readerVersion) !== owner.reader) return;
    if (owner.user === null) owner.store.setState({ value, chosen: value }, true);
    else writeViewChoice(owner.user, owner.kind, owner.key, value, owner.thing);
  }, [owner]);
  return [choice.value as T, bindDeviceChoice(choose, choice.observation), choice.observation as DeviceChoiceObservation<T> | undefined];
}

/** An explicit address wins; a plain address resumes the last valid view of this kind. */
export function useRememberedView<T extends string>(kind: string, key: string, explicit: T | undefined, fallback: T, offered: readonly T[]): [T, (value: T) => void, DeviceChoiceObservation<T>?] {
  const [kept, choose, observation] = useViewChoice<string>(kind, key, fallback);
  const available = offered.includes(kept as T);
  const address = useRef<{ scope: string; explicit: T | undefined } | null>(null);
  const scope = JSON.stringify([observation?.reader ?? requestReader(null, client.session.getState().readerVersion), kind, key]);
  useEffect(() => {
    if (address.current?.scope === scope && address.current.explicit === explicit) return;
    address.current = { scope, explicit };
    if (explicit !== undefined && available) choose(explicit);
  }, [explicit, choose, available, scope]);
  const wanted = explicit ?? kept;
  const effective = offered.includes(wanted as T) ? wanted as T : fallback;
  const fact = observation === undefined ? undefined : { ...observation, chosen: wanted, effective,
    origin: explicit !== undefined ? offered.includes(explicit) ? 'address' : 'default' : !available ? 'default' : observation.origin,
    saved_choice_availability: observation.read_result === 'invalid' ? 'unavailable' : observation.saved === undefined ? observation.saved_choice_availability : typeof observation.saved === 'string' && offered.includes(observation.saved as T) ? 'available' : 'unavailable' } satisfies DeviceChoiceObservation<T>;
  return [effective, bindDeviceChoice(choose, fact), fact];
}
