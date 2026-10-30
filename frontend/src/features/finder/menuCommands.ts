/** A menu and the palette invoke the same current entry, including its latest refusal. */
import { Pencil, type LucideIcon } from 'lucide-react';
import type { TFunction } from 'i18next';
import { useEffect, useRef } from 'react';
import { type Command, registerCommands } from '@/features/finder/commands';
import type { TranslationKey } from '@/i18n';
import { choiceName } from '@/ui/Choice';
import type { MenuEntry } from '@/ui/Menu';

const WORDS: Readonly<Partial<Record<string, TranslationKey>>> = {
  'new-thread': 'commands.words.thread', 'new-document': 'commands.words.document',
  'change-switch': 'commands.words.switch', 'change-fork-now': 'commands.words.fork', 'change-fork': 'commands.words.fork',
  'side-ask': 'commands.words.aside',
};
type Item = Extract<MenuEntry, { onSelect: () => void }>;
const actionable = (entry: MenuEntry): entry is Item => entry.type === undefined || entry.type === 'item';

export function menuCommands(entries: readonly MenuEntry[], current: () => readonly MenuEntry[], options: {
  prefix: string; name: string; skip?: ReadonlySet<string>; icons?: Readonly<Record<string, LucideIcon>>;
}): Command[] {
  return entries.flatMap((entry): Command[] => {
    if (!actionable(entry) || options.skip?.has(entry.id) === true) return [];
    const id = entry.id;
    const latest = () => current().find((item): item is Item => actionable(item) && item.id === id);
    return [{ id: `${options.prefix}.${id}`, kind: 'place', title: () => choiceName(entry),
      about: (t) => t('commands.here', { name: options.name }), icon: options.icons?.[id] ?? Pencil,
      ...(WORDS[id] === undefined ? {} : { keywords: (t: TFunction) => t(WORDS[id] ?? 'commands.words.make').split(/\s+/) }),
      ...(entry.shortcut === undefined ? {} : { shortcut: entry.shortcut }),
      available: () => { const item = latest(); return item === undefined ? false : item.disabled === undefined || item.disabled === false ? true : { disabled: item.disabled }; },
      run: () => {
        const item = latest();
        if (item !== undefined && (item.disabled === undefined || item.disabled === false)) item.onSelect();
      },
    }];
  });
}

export function useMenuCommands(entries: readonly MenuEntry[], options: Parameters<typeof menuCommands>[2]): void {
  const latest = useRef<readonly MenuEntry[]>([]);
  useEffect(() => { latest.current = entries; });
  useEffect(() => () => { latest.current = []; }, []);
  // A menu is a new array every render; only visible command metadata re-registers it.
  const signature = JSON.stringify(entries.filter(actionable).map((entry) => [entry.id, choiceName(entry), entry.disabled ?? null, entry.shortcut ?? null]));
  const { prefix, name, icons, skip } = options;
  useEffect(() => registerCommands(menuCommands(latest.current, () => latest.current, { prefix, name, icons, skip })), [signature, prefix, name, icons, skip]);
}
