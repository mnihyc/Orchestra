/** Every keyboard action of the application, the keys it ships with, and the person's own choices about them (`keys` of `GET /me/state`).
 *
 * A person may switch off the keys that need no modifier (a letter, a symbol, a sequence of them, End),
 * which dictation and a slipping hand set off by accident; and may give an action other keys, take its keys
 * away, or give it back the keys it shipped with. The list of keys is where this is done, and the menus,
 * the tooltips and the help read the same current map. Each action keeps the place it works in: a key of
 * the tree acts where the tree is shown, whatever key it is.
 *
 * The server checks a person's keys against the same actions and shipped keys (`KEY_ACTIONS` of the API
 * contract's personal state): the two lists change together.
 */
import type { TranslationKey } from '@/i18n';
import { type Chord, formatShortcut, isComposing, keyOfEvent, parseShortcut, type Shortcut } from '@/lib/shortcuts';

interface KeyAction {
  /** The keys it ships with, as people read them. */
  shipped: string;
  /** What it does, as the keys that act on a screen are listed. */
  what: TranslationKey;
  /** Its own name in the list where a key is changed, where it shares `what` with others (`next or previous area`). */
  name?: TranslationKey;
  /** The number of the tab it brings to the front, for the nine that do. */
  tab?: number;
}

const tab = (number: number): KeyAction => ({ shipped: `Alt+${number}`, what: 'shell.shortcuts.tabByNumber', name: 'shell.keyChoice.actions.tab', tab: number });

/** The actions, in the order the list shows them. */
export const KEY_ACTIONS = {
  finder: { shipped: 'Mod+K', what: 'help.keys.finder' },
  search: { shipped: 'Mod+/', what: 'help.keys.search' },
  find: { shipped: 'Mod+F', what: 'search.find.action' },
  'left-bar': { shipped: 'Mod+\\', what: 'shell.shortcuts.leftBar' },
  // Not Ctrl+.: on Windows that key switches the punctuation of Microsoft's Chinese input method.
  'right-bar': { shipped: 'Mod+Shift+.', what: 'shell.shortcuts.rightBar' },
  bottom: { shipped: 'Mod+J', what: 'shell.shortcuts.bottom' },
  'next-area': { shipped: 'F6', what: 'shell.shortcuts.areas', name: 'shell.keyChoice.actions.nextArea' },
  'previous-area': { shipped: 'Shift+F6', what: 'shell.shortcuts.areas', name: 'shell.keyChoice.actions.previousArea' },
  'tab-1': tab(1),
  'tab-2': tab(2),
  'tab-3': tab(3),
  'tab-4': tab(4),
  'tab-5': tab(5),
  'tab-6': tab(6),
  'tab-7': tab(7),
  'tab-8': tab(8),
  'tab-9': tab(9),
  'close-tab': { shipped: 'Alt+W', what: 'shell.shortcuts.closeTab' },
  'focus-mode': { shipped: '', what: 'workbench.workspace.focus' },
  help: { shipped: 'F1', what: 'shell.shortcuts.help' },
  inbox: { shipped: 'G I', what: 'shell.shortcuts.inbox' },
  approvals: { shipped: 'G A', what: 'shell.shortcuts.approvals' },
  messages: { shipped: 'G M', what: 'shell.shortcuts.messages' },
  list: { shipped: '?', what: 'shell.shortcuts.list' },
  'tree-up': { shipped: 'Alt+Shift+ArrowLeft', what: 'navigation.keys.up' },
  'tree-only': { shipped: 'Alt+Shift+ArrowRight', what: 'navigation.keys.only' },
  'tree-filter': { shipped: 'Ctrl+Alt+F', what: 'navigation.keys.filter' },
  'previous-thread': { shipped: 'Alt+ArrowUp', what: 'navigation.keys.threads', name: 'shell.keyChoice.actions.previousThread' },
  'next-thread': { shipped: 'Alt+ArrowDown', what: 'navigation.keys.threads', name: 'shell.keyChoice.actions.nextThread' },
  'previous-unread': { shipped: 'Alt+Shift+ArrowUp', what: 'navigation.keys.threads', name: 'shell.keyChoice.actions.previousUnread' },
  'next-unread': { shipped: 'Alt+Shift+ArrowDown', what: 'navigation.keys.threads', name: 'shell.keyChoice.actions.nextUnread' },
  'document-save': { shipped: 'Mod+S', what: 'documents.keys.save' },
} as const satisfies Record<string, KeyAction>;

export type KeyActionId = keyof typeof KEY_ACTIONS;

export const KEY_ACTION_IDS = Object.keys(KEY_ACTIONS) as KeyActionId[];

export function isKeyAction(id: string): id is KeyActionId {
  return Object.hasOwn(KEY_ACTIONS, id);
}

/** The action's own entry, read through the common shape. */
export function keyAction(id: KeyActionId): KeyAction {
  return KEY_ACTIONS[id];
}

/** What a person chose: whether keys without a modifier act, and their own keys by action (`null`: none). */
export interface KeyChoices {
  single_key: boolean;
  bindings: Partial<Record<KeyActionId, string | null>>;
}

export const SHIPPED_CHOICES: KeyChoices = { single_key: true, bindings: {} };

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** The keys as people read them, in the one way they are kept: `Mod+Shift+.`, `G I`. */
export function normalShortcut(text: string): string {
  return formatShortcut(parseShortcut(text));
}

/** The person's choices as kept; what this build cannot read is left as shipped, never a keyboard that does nothing. */
export function keyChoicesOf(value: unknown): KeyChoices {
  if (!isRecord(value)) return SHIPPED_CHOICES;
  const bindings: Partial<Record<KeyActionId, string | null>> = {};
  if (isRecord(value.bindings)) {
    for (const [id, binding] of Object.entries(value.bindings)) {
      if (!isKeyAction(id)) continue;
      if (binding === null) bindings[id] = null;
      else if (typeof binding === 'string' && usable(parseShortcut(binding))) bindings[id] = normalShortcut(binding);
    }
  }
  return { single_key: value.single_key !== false, bindings };
}

/** The value to keep after a change: the change made to what is kept, and every entry this build does not know left as it was. */
export function keptChoices(kept: unknown, change: { single_key?: boolean; binding?: { id: KeyActionId; to: string | null | undefined } }): Record<string, unknown> {
  const held = isRecord(kept) ? kept : {};
  let bindings: Record<string, unknown> = isRecord(held.bindings) ? held.bindings : {};
  const binding = change.binding;
  if (binding !== undefined) {
    // `undefined` gives the action back the keys it shipped with: its entry goes.
    const others = Object.entries(bindings).filter(([id]) => id !== binding.id);
    bindings = Object.fromEntries(binding.to === undefined ? others : [...others, [binding.id, binding.to]]);
  }
  return { ...held, single_key: change.single_key ?? held.single_key !== false, bindings };
}

/** The keys an action has now: its own, none, or the shipped ones. */
export function currentKeys(id: KeyActionId, choices: KeyChoices): string | null {
  const own = choices.bindings[id];
  return own === undefined ? KEY_ACTIONS[id].shipped || null : own;
}

/** Whether keys act without a modifier: a character or a sequence with one, or End. Shift that types a symbol is no modifier. */
export function isSingleKey(text: string): boolean {
  return parseShortcut(text).some((chord) => !chord.mod && !chord.ctrl && !chord.alt && (chord.key.length === 1 || chord.key === 'end'));
}

/** Whether the action's keys act now: it has keys, and they are not ones the person switched off. */
export function keysAct(id: KeyActionId, choices: KeyChoices): string | null {
  const keys = currentKeys(id, choices);
  if (keys === null) return null;
  return choices.single_key || !isSingleKey(keys) ? keys : null;
}

const NAVIGATION = new Set(['tab', 'enter', 'escape', ' ', 'space', 'backspace', 'delete', 'arrowup', 'arrowdown', 'arrowleft', 'arrowright', 'home', 'end', 'pageup', 'pagedown']);

/** Keys the browser or the system keeps for itself, written for this system: they never reach the page, or must not. */
function reservedKeys(apple: boolean): readonly string[] {
  const common = ['Mod+W', 'Mod+T', 'Mod+N', 'Mod+Q', 'Mod+R', 'Mod+L', 'Mod+P', 'Mod+Shift+T', 'Mod+Shift+N', 'Mod+Shift+W', 'Mod+Tab', 'Ctrl+Tab', 'Ctrl+Shift+Tab', 'F5', 'F11', 'F12', 'Shift+F10'];
  // Ctrl+Space and Ctrl+. belong to the input methods of Windows and Linux; Alt+Tab and Alt+F4 to the window manager.
  return apple ? [...common, 'Mod+H', 'Mod+M', 'Mod+Space', 'Mod+Alt+Escape'] : [...common, 'Ctrl+Space', 'Ctrl+.', 'Alt+Tab', 'Alt+F4'];
}

/** Whether a shortcut can be kept: one or two chords, each a key and nothing more, none a key a control moves with by itself. */
function usable(shortcut: Shortcut): boolean {
  if (shortcut.length === 0 || shortcut.length > 2) return false;
  return shortcut.every((chord) => chord.key !== '' && !(NAVIGATION.has(chord.key) && !chord.mod && !chord.ctrl && !chord.alt));
}

const same = (one: Chord, other: Chord): boolean =>
  one.key === other.key && one.mod === other.mod && one.ctrl === other.ctrl && one.alt === other.alt && one.shift === other.shift;

/** Whether `start` is where `whole` begins, or the same keys. */
function begins(whole: Shortcut, start: Shortcut): boolean {
  return start.length <= whole.length && start.every((chord, at) => {
    const theirs = whole[at];
    return theirs !== undefined && same(chord, theirs);
  });
}

/** Why keys cannot be given to an action: another action has them, or begins with them or they begin with its; the system keeps them; they are no keys. */
export type KeysRefused = { kind: 'taken' | 'prefix'; other: KeyActionId } | { kind: 'reserved' } | { kind: 'unusable' };

/** Whether the keys can be given to the action, against every other action's keys now. */
export function refusalOf(id: KeyActionId, text: string, choices: KeyChoices, apple: boolean): KeysRefused | null {
  const proposed = parseShortcut(text);
  if (!usable(proposed)) return { kind: 'unusable' };
  const written = normalShortcut(text);
  if (reservedKeys(apple).some((reserved) => normalShortcut(reserved) === written || (proposed.length > 1 && begins(proposed, parseShortcut(reserved))))) return { kind: 'reserved' };
  for (const other of KEY_ACTION_IDS) {
    if (other === id) continue;
    const theirs = currentKeys(other, choices);
    if (theirs === null) continue;
    const them = parseShortcut(theirs);
    if (them.length === proposed.length && begins(them, proposed)) return { kind: 'taken', other };
    if (begins(them, proposed) || begins(proposed, them)) return { kind: 'prefix', other };
  }
  return null;
}

/** The chord a key event makes while keys are being chosen, or `null` for a key that makes none (a modifier alone, a composing input method). */
export function chordOfEvent(event: KeyboardEvent, apple: boolean): Chord | null {
  if (isComposing(event) || ['Shift', 'Control', 'Alt', 'Meta', 'Dead', 'Unidentified', 'Process'].includes(event.key)) return null;
  const mod = apple ? event.metaKey : event.ctrlKey;
  const ctrl = apple && event.ctrlKey;
  const alt = event.altKey;
  // A symbol typed by itself is the symbol the layout types, as `?` is written; the Shift that typed it is no part of it.
  if (!mod && !ctrl && !alt && event.key.length === 1 && !/[a-z0-9]/i.test(event.key)) return { key: event.key, mod: false, ctrl: false, alt: false, shift: false };
  return { key: keyOfEvent(event), mod, ctrl, alt, shift: event.shiftKey };
}
