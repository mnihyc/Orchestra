/** The commands that screens offer: they come with the screen and go with it, a command can be offered switched off with its reason, and the list of keys is drawn from what is bound. */
import { describe, expect, it, vi } from 'vitest';

import { type Command, commandsNow, registerCommands } from '@/features/finder/commands';
import { type Binding, keyRows, ShortcutBook } from '@/lib/shortcuts';

const command = (id: string, more: Partial<Command> = {}): Command => ({ id, kind: 'go', title: () => id, run: () => undefined, ...more });

describe('the commands a screen offers', () => {
  it('are listed while the screen is in view and gone when it goes', () => {
    const away = registerCommands([command('thread.fork'), command('thread.find')]);
    expect(commandsNow().map((one) => one.id)).toEqual(expect.arrayContaining(['thread.fork', 'thread.find']));
    away();
    expect(commandsNow().map((one) => one.id)).not.toContain('thread.fork');
  });

  it('are replaced by a command of the same name, and taking the first registration away does not take the new one', () => {
    const first = command('place.rename', { title: () => 'Rename…' });
    const second = command('place.rename', { title: () => 'Rename this…' });
    const awayFirst = registerCommands([first]);
    const awaySecond = registerCommands([second]);
    expect(commandsNow().find((one) => one.id === 'place.rename')?.title({} as never)).toBe('Rename this…');
    awayFirst();
    expect(commandsNow().some((one) => one.id === 'place.rename')).toBe(true);
    awaySecond();
    expect(commandsNow().some((one) => one.id === 'place.rename')).toBe(false);
  });

  it('say whether they can be used: yes, not at all, or no with the reason', () => {
    const run = vi.fn();
    const away = registerCommands([
      command('a', { available: () => true, run }),
      command('b', { available: () => false }),
      command('c', { available: () => ({ disabled: 'Forking this thread is for its Developers' }) }),
    ]);
    const held = Object.fromEntries(commandsNow().map((one) => [one.id, one.available?.() ?? true]));
    expect(held.a).toBe(true);
    expect(held.b).toBe(false);
    expect(held.c).toEqual({ disabled: 'Forking this thread is for its Developers' });
    away();
  });
});

describe('the list of keys', () => {
  const book = () => {
    const keys = new ShortcutBook(false);
    keys.add('finder', 'Mod+K', () => true, 'help.keys.finder');
    keys.add('next-area', 'F6', () => true, 'shell.shortcuts.areas');
    keys.add('previous-area', 'Shift+F6', () => true, 'shell.shortcuts.areas');
    for (let index = 1; index <= 9; index += 1) keys.add(`tab-${index}`, `Alt+${index}`, () => true, 'shell.shortcuts.tabByNumber');
    keys.add('quiet', 'Mod+Shift+Q', () => true);
    return keys;
  };

  it('is made from the keys that are bound and says nothing else', () => {
    const rows = keyRows(book().list());
    expect(rows).toEqual([
      { what: 'help.keys.finder', first: 'Mod+K', last: null },
      { what: 'shell.shortcuts.areas', first: 'F6', last: 'Shift+F6' },
      { what: 'shell.shortcuts.tabByNumber', first: 'Alt+1', last: 'Alt+9' },
    ]);
  });

  it('gains a line when a screen binds a key of its own and loses it when the screen goes', () => {
    const keys = book();
    const away = keys.add('file-save', 'Mod+S', () => true, 'help.keys.search');
    expect(keyRows(keys.list()).map((row) => row.what)).toContain('help.keys.search');
    away();
    expect(keyRows(keys.list()).map((row) => row.what)).not.toContain('help.keys.search');
  });

  it('leaves out a key that does not say what it does, and writes the keys as they were bound', () => {
    const bindings: Binding[] = book().list();
    expect(bindings.find((one) => one.id === 'quiet')?.text).toBe('Mod+Shift+Q');
    expect(keyRows(bindings).some((row) => row.first === 'Mod+Shift+Q')).toBe(false);
  });

  it('is what the keys really do: a bound key is taken and an unbound one is not', () => {
    const keys = new ShortcutBook(false);
    const press = (key: string, init: KeyboardEventInit = {}) => new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init });
    const used = vi.fn();
    keys.add('finder', 'Mod+K', used, 'help.keys.finder');
    expect(keys.handle(press('k', { ctrlKey: true }))).toBe(true);
    expect(used).toHaveBeenCalledTimes(1);
    expect(keys.handle(press('j', { ctrlKey: true }))).toBe(false);
  });
});
