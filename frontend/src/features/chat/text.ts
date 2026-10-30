/** What a person types and what is sent: the word being mentioned under the caret, a mention put in its place, and the text of a message as parts.
 *
 * The composer keeps plain text, and a message is parts. These are the few conversions between them,
 * kept apart from the composer so that the rules (a mention begins a word; a message of blanks says
 * nothing; an edit keeps what is not text) are held by tests that need no page.
 */
import type { ChatPart } from '@/generated/api-types';

export interface MentionQuery {
  /** Where the `@` is. */
  start: number;
  /** Where the caret is: the end of what has been typed of the name. */
  end: number;
  /** What has been typed of the name, without the `@`. */
  query: string;
}

/** The characters a username is made of; the same set the server takes after an `@`. */
const NAME_CHARS = /^[a-z0-9_-]*$/i;

/** The `@word` the caret is at the end of, when the `@` begins a word. */
export function mentionAt(text: string, caret: number): MentionQuery | null {
  const before = text.slice(0, caret);
  const at = before.lastIndexOf('@');
  if (at < 0) return null;
  const query = before.slice(at + 1);
  if (!NAME_CHARS.test(query)) return null;
  // A mention begins a word: an address such as ada@example.org is no mention.
  const lead = at === 0 ? '' : before.charAt(at - 1);
  if (lead !== '' && !/\s/.test(lead)) return null;
  return { start: at, end: caret, query };
}

/** The text with the mention under the caret replaced by `@username` and a space, and where the caret goes. */
export function insertMention(text: string, mention: MentionQuery, username: string): { text: string; caret: number } {
  const inserted = `@${username} `;
  const after = text.slice(mention.end);
  const joined = `${text.slice(0, mention.start)}${inserted}${after.startsWith(' ') ? after.slice(1) : after}`;
  return { text: joined, caret: mention.start + inserted.length };
}

/** Whether a text says nothing: nothing, or only white space. */
export function isBlank(text: string): boolean {
  return text.trim() === '';
}

/** A message's parts for a text: one text part, or none for a blank text. */
export function textPart(text: string): ChatPart[] {
  return isBlank(text) ? [] : [{ type: 'text', text: text.trim() }];
}

/** The text of a message's text parts, one under the other. */
export function textOfParts(parts: readonly ChatPart[]): string {
  return parts
    .map((part) => ('type' in part && part.type === 'text' ? part.text : null))
    .filter((text): text is string => text !== null)
    .join('\n\n');
}

/** The parts of an edited message: the new text first, then everything that is not text as it was. */
export function editedParts(original: readonly ChatPart[], text: string): ChatPart[] {
  const rest = original.filter((part) => !('type' in part && part.type === 'text'));
  return [...textPart(text), ...rest];
}

/** A line of Markdown as plain words: the marks of emphasis, code and quotation taken off, a link left as its text. */
export function plain(line: string): string {
  return line
    .replace(/\[([^\]]+)\]\([^)\s]+\)/g, '$1')
    .replace(/(\*\*|__|~~|`)/g, '')
    .replace(/(^|\s)[*_]([^*_\s][^*_]*)[*_](?=\s|$)/g, '$1$2')
    .replace(/^\s{0,3}(?:>\s*|[-*]\s+|\d{1,9}[.)]\s+)/, '');
}

/** The first line of a message, for a reply bar and a row of a list: what it says, as plain words, cut to a length. */
export function firstLine(parts: readonly ChatPart[], limit: number): string {
  const text = textOfParts(parts).trim();
  const line = text.split('\n').find((one) => one.trim() !== '') ?? '';
  return Array.from(plain(line)).slice(0, limit).join('');
}
