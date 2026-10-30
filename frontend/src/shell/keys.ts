/** The one book of keyboard shortcuts of the application: what is bound at this moment, so that the list of keys is read from it.
 *
 * The shell listens to the page once and gives every key to this book. The shell adds the keys that
 * work everywhere; a screen or a view that has keys of its own adds them while it is shown (`useKeyBinding`)
 * and they are gone when it goes. The list that `?` opens and the help view's list are made from the
 * bindings that say what they do, so a key that is bound is listed and a key that is listed is bound.
 */
import { useEffect, useSyncExternalStore } from 'react';

import { type Binding, ShortcutBook } from '@/lib/shortcuts';
import type { TranslationKey } from '@/i18n';
import { isKeyAction, isSingleKey, KEY_ACTIONS, type KeyActionId, type KeyChoices, keysAct, SHIPPED_CHOICES } from '@/shell/keyActions';

/** What the shell and the screens bind their keys in. */
export const keyBook = new ShortcutBook();

const listeners = new Set<() => void>();
let snapshot: readonly Binding[] = [];
let askedSnapshot: readonly string[] = [];

/** Takes the bindings as they stand now, and tells whoever shows them. */
export function noteBindingsChanged(): void {
  snapshot = keyBook.list();
  askedSnapshot = [...asked.keys()];
  for (const listener of listeners) listener();
}

// ---------------------------------------------------------------- the person's choices

/** An action as a screen asked for it: the keys it ships with and what it does; the book holds it under the keys it has now. */
interface Asked {
  shipped: string;
  run: Binding['run'];
  what: TranslationKey | undefined;
  remove: (() => void) | null;
}

const asked = new Map<string, Asked>();
let choices: KeyChoices = SHIPPED_CHOICES;
const choiceListeners = new Set<() => void>();

/** Puts an action into the book under the keys it has now, or leaves it out when it has none or they are switched off. */
function place(id: string, entry: Asked): void {
  entry.remove?.();
  entry.remove = null;
  const keys = isKeyAction(id) ? keysAct(id, choices) : choices.single_key || !isSingleKey(entry.shipped) ? entry.shipped : null;
  if (keys !== null) entry.remove = keyBook.add(id, keys, entry.run, entry.what);
}

/** Takes the person's choices (`keys` of their state): every action is bound again under the keys it has now. */
export function setKeyChoices(next: KeyChoices): void {
  if (next === choices) return;
  choices = next;
  for (const [id, entry] of asked) place(id, entry);
  noteBindingsChanged();
  for (const listener of choiceListeners) listener();
}

function subscribeChoices(listener: () => void): () => void {
  choiceListeners.add(listener);
  return () => choiceListeners.delete(listener);
}

/** The person's choices as they stand. */
export function useKeyChoices(): KeyChoices {
  return useSyncExternalStore(subscribeChoices, () => choices);
}

/** The keys that do an action now, for a tooltip, a menu or the help to show; `undefined` where none do. */
export function useActionKeys(id: KeyActionId): string | undefined {
  return keysAct(id, useKeyChoices()) ?? undefined;
}

/** The same, read once, for a menu made outside a component. */
export function actionKeys(id: KeyActionId): string | undefined {
  return keysAct(id, choices) ?? undefined;
}

/** Adds a binding to the book and tells the list of keys; what it returns takes the binding away again. */
export function bindKey(id: string, shortcut: string, run: Binding['run'], what?: TranslationKey): () => void {
  const entry: Asked = { shipped: shortcut, run, what, remove: null };
  asked.get(id)?.remove?.();
  asked.set(id, entry);
  place(id, entry);
  noteBindingsChanged();
  return () => {
    entry.remove?.();
    entry.remove = null;
    if (asked.get(id) === entry) asked.delete(id);
    noteBindingsChanged();
  };
}

/** Binds one of the application's actions under the keys it has now. */
export function bindAction(id: KeyActionId, run: Binding['run']): () => void {
  return bindKey(id, KEY_ACTIONS[id].shipped, run, KEY_ACTIONS[id].what);
}

/** Binds a key for as long as the calling component is shown. `run` may change between renders. */
export function useKeyBinding(id: string, shortcut: string, run: Binding['run'], what?: TranslationKey): void {
  useEffect(() => bindKey(id, shortcut, run, what), [id, shortcut, run, what]);
}

/** Binds one of the application's actions for as long as the calling component is shown. */
export function useKeyAction(id: KeyActionId, run: Binding['run']): void {
  useEffect(() => bindAction(id, run), [id, run]);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** The bindings in force, following them as screens add and take away their own. */
export function useBindings(): readonly Binding[] {
  return useSyncExternalStore(subscribe, () => snapshot);
}

/** The actions the screens shown now ask for, whether or not their keys act: what acts here, for the list of keys. */
export function useAskedActions(): readonly string[] {
  return useSyncExternalStore(subscribe, () => askedSnapshot);
}

// ---------------------------------------------------------------- the list of keys

const listRequests = new Set<() => void>();

/** Asks for the list of keys to be shown: the shell draws it, and a command of the finder asks for it. */
export function requestKeyList(): void {
  for (const request of listRequests) request();
}

/** Calls `show` whenever somebody asks for the list of keys. */
export function onKeyListRequested(show: () => void): () => void {
  listRequests.add(show);
  return () => listRequests.delete(show);
}
