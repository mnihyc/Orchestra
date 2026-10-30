/** A person's own keys: single keys switched off, an action given other keys, none, or its shipped ones back; and the keys that cannot be chosen. */
import { afterEach, describe, expect, it } from 'vitest';

import {
  chordOfEvent,
  currentKeys,
  isSingleKey,
  KEY_ACTION_IDS,
  KEY_ACTIONS,
  keyChoicesOf,
  keptChoices,
  keysAct,
  refusalOf,
  SHIPPED_CHOICES,
} from '@/shell/keyActions';
import { bindAction, keyBook, setKeyChoices } from '@/shell/keys';

const key = (init: KeyboardEventInit & { key: string; code?: string }) => new KeyboardEvent('keydown', init);

describe('what is kept', () => {
  it('reads the switch and the keys of known actions, in the one way keys are written', () => {
    expect(keyChoicesOf({ single_key: false, bindings: { finder: 'mod+p', inbox: null, unknown: 'Mod+U', search: 42 } })).toEqual({ single_key: false, bindings: { finder: 'Mod+P', inbox: null } });
  });

  it('acts as shipped where nothing can be read', () => {
    expect(keyChoicesOf(undefined)).toBe(SHIPPED_CHOICES);
    expect(keyChoicesOf('on')).toBe(SHIPPED_CHOICES);
    expect(keyChoicesOf({ bindings: { finder: 'ArrowUp' } })).toEqual({ single_key: true, bindings: {} });
  });

  it('changes only what it is asked to, and keeps what this build does not know', () => {
    const kept = { single_key: true, bindings: { finder: 'Mod+P', 'future-action': 'Mod+Shift+U' }, note: 'kept' };
    expect(keptChoices(kept, { binding: { id: 'inbox', to: null } })).toEqual({ single_key: true, bindings: { finder: 'Mod+P', 'future-action': 'Mod+Shift+U', inbox: null }, note: 'kept' });
    expect(keptChoices(kept, { binding: { id: 'finder', to: undefined } })).toEqual({ single_key: true, bindings: { 'future-action': 'Mod+Shift+U' }, note: 'kept' });
    expect(keptChoices(kept, { single_key: false })).toMatchObject({ single_key: false, bindings: kept.bindings });
    expect(keptChoices(undefined, { single_key: false })).toEqual({ single_key: false, bindings: {} });
  });
});

describe('single keys', () => {
  it('are the keys without Ctrl, Alt or Command: letters, symbols, sequences of them and End', () => {
    for (const keys of ['G I', '?', '/', 'End', 'Shift+?']) expect(isSingleKey(keys), keys).toBe(true);
    for (const keys of ['Mod+K', 'F1', 'Shift+F6', 'Alt+1', 'Ctrl+Alt+F', 'Mod+Shift+.']) expect(isSingleKey(keys), keys).toBe(false);
  });

  it('switched off, stop acting, while an action moved onto a modifier still acts', () => {
    const off = { single_key: false, bindings: { list: 'Mod+Shift+/' } };
    expect(keysAct('inbox', off)).toBeNull();
    expect(keysAct('list', off)).toBe('Mod+Shift+/');
    expect(keysAct('finder', off)).toBe('Mod+K');
    expect(currentKeys('inbox', off)).toBe('G I');
  });

  it('leave the book when switched off, and come back when switched on', () => {
    const remove = bindAction('inbox', () => true);
    try {
      expect(keyBook.list().some((binding) => binding.id === 'inbox')).toBe(true);
      setKeyChoices({ single_key: false, bindings: {} });
      expect(keyBook.list().some((binding) => binding.id === 'inbox')).toBe(false);
      setKeyChoices({ single_key: true, bindings: { inbox: 'G N' } });
      expect(keyBook.list().find((binding) => binding.id === 'inbox')?.text).toBe('G N');
    } finally {
      remove();
      setKeyChoices(SHIPPED_CHOICES);
    }
  });
});

describe('choosing keys', () => {
  afterEach(() => setKeyChoices(SHIPPED_CHOICES));

  it('the right bar is not Ctrl+., the punctuation switch of the Chinese input method on Windows', () => {
    expect(KEY_ACTIONS['right-bar'].shipped).toBe('Mod+Shift+.');
    expect(refusalOf('right-bar', 'Ctrl+.', SHIPPED_CHOICES, false)).toEqual({ kind: 'reserved' });
  });

  it('refuses keys another action has, naming it, and keys that begin or are the beginning of another', () => {
    expect(refusalOf('search', 'Mod+K', SHIPPED_CHOICES, false)).toEqual({ kind: 'taken', other: 'finder' });
    expect(refusalOf('search', 'G', SHIPPED_CHOICES, false)).toEqual({ kind: 'prefix', other: 'inbox' });
    expect(refusalOf('search', 'Mod+K X', SHIPPED_CHOICES, false)).toEqual({ kind: 'prefix', other: 'finder' });
    // Keys another action no longer has are free.
    expect(refusalOf('search', 'Mod+K', { single_key: true, bindings: { finder: null } }, false)).toBeNull();
  });

  it('refuses what the browser or the system keeps, what a control moves with, and more than two keys in a row', () => {
    expect(refusalOf('finder', 'Mod+W', SHIPPED_CHOICES, false)).toEqual({ kind: 'reserved' });
    expect(refusalOf('finder', 'Mod+W K', SHIPPED_CHOICES, false)).toEqual({ kind: 'reserved' });
    expect(refusalOf('finder', 'ArrowUp', SHIPPED_CHOICES, false)).toEqual({ kind: 'unusable' });
    expect(refusalOf('finder', 'G O T', SHIPPED_CHOICES, false)).toEqual({ kind: 'unusable' });
    expect(refusalOf('finder', 'Mod+Shift+Y', SHIPPED_CHOICES, false)).toBeNull();
  });

  it('as shipped, no two actions share keys or begin one another', () => {
    for (const id of KEY_ACTION_IDS) {
      const shipped = KEY_ACTIONS[id].shipped;
      if (shipped === '') expect(currentKeys(id, SHIPPED_CHOICES), id).toBeNull();
      else expect(refusalOf(id, shipped, SHIPPED_CHOICES, false), id).toBeNull();
    }
  });

  it('takes a symbol typed with Shift as the symbol, and a chord with a modifier by its key', () => {
    expect(chordOfEvent(key({ key: '?', code: 'Slash', shiftKey: true }), false)).toEqual({ key: '?', mod: false, ctrl: false, alt: false, shift: false });
    expect(chordOfEvent(key({ key: '>', code: 'Period', ctrlKey: true, shiftKey: true }), false)).toEqual({ key: '.', mod: true, ctrl: false, alt: false, shift: true });
    expect(chordOfEvent(key({ key: 'Shift', code: 'ShiftLeft', shiftKey: true }), false)).toBeNull();
    expect(chordOfEvent(key({ key: 'k', code: 'KeyK', metaKey: true }), true)).toEqual({ key: 'k', mod: true, ctrl: false, alt: false, shift: false });
  });
});

it('keeps Focus mode assignable without inventing a shipped keyboard binding', () => {
  expect(KEY_ACTIONS['focus-mode'].shipped).toBe('');
  const remove = bindAction('focus-mode', () => true);
  try {
    setKeyChoices(SHIPPED_CHOICES);
    expect(keysAct('focus-mode', SHIPPED_CHOICES)).toBeNull();
    expect(keyBook.list().some((entry) => entry.id === 'focus-mode')).toBe(false);
    const choices = keyChoicesOf({ bindings: { 'focus-mode': 'Ctrl+Shift+F8' } });
    expect(refusalOf('focus-mode', 'Ctrl+Shift+F8', SHIPPED_CHOICES, false)).toBeNull();
    setKeyChoices(choices);
    expect(keyBook.list().find((entry) => entry.id === 'focus-mode')?.text).toBe('Ctrl+Shift+F8');
  } finally { remove(); setKeyChoices(SHIPPED_CHOICES); }
});
