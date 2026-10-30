/** Match the action's words and the keys actually shown on this keyboard; no request or former binding. */
import { shortcutCaps } from '@/lib/shortcuts';

const normal = (text: string): string => text.normalize('NFKC').toLocaleLowerCase()
  .replace(/command|cmd|⌘/g, 'command').replace(/control|ctrl|⌃/g, 'ctrl').replace(/shift|⇧/g, 'shift').replace(/option|opt|⌥/g, 'alt')
  .replace(/\+/g, ' ').replace(/\s+/g, ' ').trim();

export function shortcutMatches(query: string, row: { name: string; keys: readonly string[]; where: string }, apple?: boolean): boolean {
  const parts = [row.name, row.where, ...row.keys.flatMap((keys) => [keys, shortcutCaps(keys, apple).flat().join(' ')])].map(normal);
  return normal(query).split(' ').filter((word) => word !== '').every((word) => parts.some((part) => part.includes(word)));
}
