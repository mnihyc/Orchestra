/** What is said to an agent, as the parts it is made of: words, images, files, and messages forwarded from elsewhere.
 *
 * Every part carries its discriminator in the contract, including a forwarded message by itself.
 */
import type { ContentPart } from '@/generated/api-types';

/** A message forwarded into the thread. */
export type ForwardedPart = Extract<ContentPart, { type: 'forwarded' }>;
/** A part that is words, an image or a file. */
export type PlainPart = Exclude<ContentPart, ForwardedPart>;

/** Whether a part is a message forwarded from elsewhere: of what is said to an agent as of what is said in a chat. */
export function isForwarded<Part extends object>(part: Part): part is Extract<Part, { type: 'forwarded' }> {
  return 'type' in part && part.type === 'forwarded';
}

/** The words of a message, its text parts joined by line. */
export function wordsOf(parts: readonly ContentPart[]): string {
  return parts.flatMap((part) => (!isForwarded(part) && part.type === 'text' ? [part.text] : [])).join('\n');
}

/** The first line that says something, for a row that has room for one. */
export function firstLineOfParts(parts: readonly ContentPart[]): string {
  for (const part of parts) {
    if (!isForwarded(part) && part.type === 'text' && part.text.trim() !== '') return part.text.trim().split('\n', 1)[0] ?? '';
  }
  return parts.length === 0 ? '' : '…';
}
