/** The modes of the finder: a sign at the start of the field keeps it to one kind of answer.
 *
 * With no sign the finder looks everywhere. `>` keeps it to commands, `@` to people, `:` to settings and
 * the rows of pages, `?` to help, and `/` turns the words into a search of what was said. The sign is
 * part of what is typed, so it is seen, kept and removed like any letter, and the buttons under the field
 * only write or take away the same sign.
 */

export type Mode = 'all' | 'commands' | 'people' | 'settings' | 'help' | 'conversations';

/** The sign of each mode that has one. */
export const SIGNS: Record<Exclude<Mode, 'all'>, string> = {
  commands: '>',
  people: '@',
  settings: ':',
  help: '?',
  conversations: '/',
};

/** The modes in the order the buttons under the field list them. */
export const MODES: readonly Mode[] = ['all', 'commands', 'people', 'settings', 'help', 'conversations'];

export interface Field {
  mode: Mode;
  /** What is looked for: what was typed, without the sign. */
  query: string;
}

/** Reads what is typed: the mode its first character chooses, and the words after it. */
export function parseField(text: string): Field {
  const first = text.charAt(0);
  for (const mode of MODES) {
    if (mode === 'all') continue;
    if (SIGNS[mode] === first) return { mode, query: text.slice(1).trim() };
  }
  return { mode: 'all', query: text.trim() };
}

/** What the field reads after the reader chose a mode with a button: the sign and the words they had typed, in place of the sign they had. */
export function withMode(text: string, mode: Mode): string {
  const { query } = parseField(text);
  if (mode === 'all') return query;
  return query === '' ? SIGNS[mode] : `${SIGNS[mode]} ${query}`;
}

/** Whether a sign alone has been typed: the mode is chosen and nothing is asked in it yet. */
export function isBare(text: string): boolean {
  const { mode, query } = parseField(text);
  return mode !== 'all' && query === '';
}
