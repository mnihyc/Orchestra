/** Keyboard matching including IME and focus rules.
 *
 * A shortcut is written as people read it: `Mod+K`, `Shift+F6`, `Alt+1`, `G I` (two keys in a row).
 * `Mod` is Command on Apple's systems and Control elsewhere.
 *
 * The rules that keep typing safe:
 * - nothing matches while an input method is composing a character;
 * - a key that types (a letter, `/`, `?`, `End`) is never a shortcut while the focus is in a field or
 *   in something that takes the keyboard for itself, such as a terminal;
 * - a letter is the one printed on the key in the layout in use; where the layout types another
 *   script, or Alt turned the key into another character, the key is taken by its place instead.
 */

import type { TranslationKey } from '@/i18n';

export interface Chord {
  /** The key, lower case for a letter: `k`, `1`, `f6`, `arrowup`, `\`, `?`. */
  key: string;
  mod: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
}

/** One chord, or several pressed one after another. */
export type Shortcut = readonly Chord[];

/** How long the first key of a sequence waits for the second: time to find a key, not to forget one. */
export const SEQUENCE_WINDOW_MS = 1_000;

const MODIFIERS = new Set(['mod', 'ctrl', 'alt', 'shift']);

/** Reads `Mod+K`, `Shift+F6` or `G I` into chords. */
export function parseShortcut(text: string): Shortcut {
  return text
    .trim()
    .split(/\s+/)
    .map((part) => {
      // A literal plus is written last: `Mod++`.
      const tokens = part === '+' ? ['+'] : part.replace(/\+\+$/, '+plus').split('+');
      const chord: Chord = { key: '', mod: false, ctrl: false, alt: false, shift: false };
      for (const token of tokens) {
        const name = token.toLowerCase();
        if (MODIFIERS.has(name)) chord[name as 'mod' | 'ctrl' | 'alt' | 'shift'] = true;
        else chord.key = name === 'plus' ? '+' : name;
      }
      return chord;
    });
}

/** Whether this is one of Apple's systems, where Command is the modifier of shortcuts. */
export function isApplePlatform(): boolean {
  if (typeof navigator === 'undefined') return false;
  return /mac|iphone|ipad|ipod/i.test(navigator.userAgent);
}

/** The characters printed on the keys that are named by their place, on the layout shortcuts are written for. */
const PLACE_SYMBOLS: Record<string, string> = {
  Backslash: '\\',
  IntlBackslash: '\\',
  Period: '.',
  Slash: '/',
  Comma: ',',
  Semicolon: ';',
  Quote: "'",
  BracketLeft: '[',
  BracketRight: ']',
  Minus: '-',
  Equal: '=',
  Backquote: '`',
};

/**
 * The key of an event as a shortcut names it. A Latin letter or a digit is taken as the layout types
 * it, so that a shortcut is the letter printed on the key. Anything else (another script, or what Alt
 * made of the key) is taken by the place of the key instead.
 */
function keyOf(event: KeyboardEvent): string {
  const typed = event.key.toLowerCase();
  if (/^[a-z0-9]$/.test(typed) || typed.length > 1) return typed;
  const place = /^(?:Key([A-Z])|Digit([0-9]))$/.exec(event.code);
  if (place) return (place[1] ?? place[2] ?? '').toLowerCase();
  return PLACE_SYMBOLS[event.code] ?? typed;
}

/** The key of an event as a shortcut names it, for whoever records keys a person presses (the list of keys, choosing keys). */
export const keyOfEvent = keyOf;

/** Whether an input method is composing: the key belongs to it and to nothing else. */
export function isComposing(event: KeyboardEvent): boolean {
  // 229 is what browsers report for a key the input method consumed, also where `isComposing` is not
  // set yet or no longer (the key that ends a composition in Safari). Nothing newer says the same.
  // eslint-disable-next-line @typescript-eslint/no-deprecated
  return event.isComposing || event.keyCode === 229;
}

/** Whether a chord types a character or moves a caret when it reaches a field. */
function typesInField(chord: Chord, apple: boolean): boolean {
  if (chord.mod || chord.ctrl) return false;
  if (/^f\d{1,2}$/.test(chord.key) || chord.key === 'escape') return false;
  // Alt with a key types a character on Apple's systems, and nothing elsewhere.
  if (chord.alt) return apple;
  return true;
}

/** Whether the focus is somewhere that takes plain keys for itself. */
export function takesTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  if (target.closest('[data-keys="own"]')) return true;
  const tag = target.tagName;
  if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
  if (tag !== 'INPUT') return false;
  const type = (target as HTMLInputElement).type;
  return !['button', 'checkbox', 'radio', 'range', 'submit', 'reset', 'file', 'color', 'image'].includes(type);
}

/** Whether a key event is this chord, under the rules above. */
export function matchesChord(event: KeyboardEvent, chord: Chord, apple: boolean = isApplePlatform()): boolean {
  if (isComposing(event)) return false;
  if (typesInField(chord, apple) && takesTyping(event.target)) return false;

  const wantMeta = chord.mod && apple;
  const wantCtrl = chord.ctrl || (chord.mod && !apple);
  if (event.metaKey !== wantMeta || event.ctrlKey !== wantCtrl || event.altKey !== chord.alt) return false;

  // A character typed by itself (`?`, `/`) is whatever the layout makes of its keys, Shift included.
  const symbol = chord.key.length === 1 && !/[a-z0-9]/.test(chord.key);
  if (symbol && !chord.mod && !chord.ctrl && !chord.alt) return event.key === chord.key;
  return keyOf(event) === chord.key && event.shiftKey === chord.shift;
}

const APPLE_GLYPHS: Record<string, string> = { mod: '⌘', ctrl: '⌃', alt: '⌥', shift: '⇧' };
const KEY_GLYPHS: Record<string, string> = {
  arrowup: '↑',
  arrowdown: '↓',
  arrowleft: '←',
  arrowright: '→',
  enter: '↵',
  escape: 'Esc',
  ' ': 'Space',
  space: 'Space',
  end: 'End',
  home: 'Home',
  tab: 'Tab',
};

/**
 * The key caps of a shortcut, one list for each chord: `[['⌘', 'K']]` on a Mac, `[['Ctrl', 'K']]`
 * elsewhere, `[['G'], ['I']]` for a sequence. Names of keys are the ones printed on keyboards and are
 * the same in every language.
 */
export function shortcutCaps(shortcut: Shortcut | string, apple: boolean = isApplePlatform()): string[][] {
  const chords = typeof shortcut === 'string' ? parseShortcut(shortcut) : shortcut;
  return chords.map((chord) => {
    const caps: string[] = [];
    for (const modifier of ['ctrl', 'mod', 'alt', 'shift'] as const) {
      if (!chord[modifier]) continue;
      if (apple) caps.push(APPLE_GLYPHS[modifier] ?? modifier);
      else caps.push(modifier === 'mod' || modifier === 'ctrl' ? 'Ctrl' : modifier === 'alt' ? 'Alt' : 'Shift');
    }
    const glyph = KEY_GLYPHS[chord.key];
    caps.push(glyph ?? (chord.key.length === 1 ? chord.key.toUpperCase() : chord.key.replace(/^f(\d)/, 'F$1')));
    return caps;
  });
}

export interface Binding {
  /** A stable name, by which a binding is listed and replaced. */
  id: string;
  shortcut: Shortcut;
  /** The shortcut as it is written for people: `Mod+K`, `G I`. */
  text: string;
  /** What it does, in the catalogue's words, for the list of keys; a binding that says nothing is not listed. */
  what?: TranslationKey;
  /** Does it. Returning `false` says that nothing was done, and the key goes on to the page. */
  run: (event: KeyboardEvent) => unknown;
}

/** A chord written as people read it, the reverse of `parseShortcut`. */
export function formatShortcut(shortcut: Shortcut): string {
  return shortcut
    .map((chord) => {
      const key = chord.key.length === 1 ? chord.key.toUpperCase() : chord.key.replace(/^f(\d)/, 'F$1').replace(/^[a-z]/, (letter) => letter.toUpperCase());
      return [chord.mod ? 'Mod' : '', chord.ctrl ? 'Ctrl' : '', chord.alt ? 'Alt' : '', chord.shift ? 'Shift' : '', key].filter((part) => part !== '').join('+');
    })
    .join(' ');
}

/** One line of the list of keys: what the keys do, and the keys. */
export interface KeyRow {
  what: TranslationKey;
  /** The first key that does it, and the last where several do (`Alt+1` to `Alt+9`, `F6` and `Shift+F6`). */
  first: string;
  last: string | null;
}

/**
 * The list of keys, made from the bindings in force and from nothing else: bindings that say the same
 * thing are one line, in the order the first of them was added. A key that is bound is therefore listed,
 * and a key that is listed is bound.
 */
export function keyRows(bindings: readonly Binding[]): KeyRow[] {
  const rows = new Map<TranslationKey, KeyRow>();
  for (const binding of bindings) {
    if (binding.what === undefined) continue;
    const row = rows.get(binding.what);
    if (row === undefined) rows.set(binding.what, { what: binding.what, first: binding.text, last: null });
    else row.last = binding.text;
  }
  return [...rows.values()];
}

/**
 * The shortcuts that are in force, and the key that a sequence is waiting on. One of these listens to
 * the page; views add their bindings while they are shown and take them away when they go.
 */
export class ShortcutBook {
  private readonly bindings = new Map<string, Binding>();
  private pending: { bindings: Binding[]; at: number; depth: number } | null = null;
  private readonly apple: boolean;
  private readonly now: () => number;

  constructor(
    apple: boolean = isApplePlatform(),
    now: () => number = () => Date.now(),
  ) {
    this.apple = apple;
    this.now = now;
  }

  /** Adds a binding; what it returns takes it away again. */
  add(id: string, shortcut: string | Shortcut, run: Binding['run'], what?: TranslationKey): () => void {
    const parsed = typeof shortcut === 'string' ? parseShortcut(shortcut) : shortcut;
    const binding: Binding = {
      id,
      shortcut: parsed,
      text: typeof shortcut === 'string' ? shortcut : formatShortcut(shortcut),
      ...(what === undefined ? {} : { what }),
      run,
    };
    this.bindings.set(id, binding);
    return () => {
      if (this.bindings.get(id) === binding) this.bindings.delete(id);
    };
  }

  /** Every binding in force, for the list of shortcuts. */
  list(): Binding[] {
    return [...this.bindings.values()];
  }

  /** Gives a key to the bindings. Returns whether one of them took it. */
  handle(event: KeyboardEvent): boolean {
    // A focused control handles its own keys before they bubble to the workbench.
    if (event.defaultPrevented || isComposing(event)) return false;
    if (['Shift', 'Control', 'Alt', 'Meta'].includes(event.key)) return false;

    if (this.pending && this.now() - this.pending.at <= SEQUENCE_WINDOW_MS) {
      const { bindings, depth } = this.pending;
      this.pending = null;
      const next = bindings.filter((binding) => {
        const chord = binding.shortcut[depth];
        return chord !== undefined && matchesChord(event, chord, this.apple);
      });
      if (this.settle(event, next, depth)) return true;
    }
    this.pending = null;

    const first = [...this.bindings.values()].filter((binding) => {
      const chord = binding.shortcut[0];
      return chord !== undefined && matchesChord(event, chord, this.apple);
    });
    return this.settle(event, first, 0);
  }

  private settle(event: KeyboardEvent, matched: Binding[], depth: number): boolean {
    const complete = matched.filter((binding) => binding.shortcut.length === depth + 1);
    for (const binding of complete) {
      if (binding.run(event) !== false) {
        event.preventDefault();
        return true;
      }
    }
    const longer = matched.filter((binding) => binding.shortcut.length > depth + 1);
    if (longer.length > 0) {
      this.pending = { bindings: longer, at: this.now(), depth: depth + 1 };
      event.preventDefault();
      return true;
    }
    return false;
  }
}
