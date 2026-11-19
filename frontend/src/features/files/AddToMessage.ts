/** A file, some lines, a difference or a selection of a shell's output put into the conversation's composer as a reference.
 *
 * The reference stands in the box of the thread's composer, where the person can read it and word
 * the rest round it; several add up, one under the other, and they go with the next message as its
 * text. The agent gets the path, the lines and the words.
 */
import { composerWriter, useComposerStore } from '@/features/composer/Draft';

/** What is put into the message. */
export type Reference =
  | { kind: 'file'; path: string }
  | { kind: 'lines'; path: string; fromLine: number; toLine: number; text: string }
  | { kind: 'difference'; path: string; text: string }
  | { kind: 'output'; command: string | null; text: string };

/** The most characters of a quotation that go into the box: a screenful, not a file. */
export const QUOTE_AT_MOST = 4_000;

/** Puts text in a fence long enough that nothing inside it can end it. */
function fenced(text: string, language = ''): string {
  let ticks = '```';
  while (text.includes(ticks)) ticks += '`';
  return `${ticks}${language}\n${text}\n${ticks}`;
}

const cut = (text: string): string => (text.length > QUOTE_AT_MOST ? `${text.slice(0, QUOTE_AT_MOST)}\n…` : text);

/** The reference as the text that stands in the box. */
export function referenceText(reference: Reference): string {
  switch (reference.kind) {
    case 'file':
      return `\`${reference.path}\``;
    case 'lines': {
      const range = reference.fromLine === reference.toLine ? `${reference.fromLine}` : `${reference.fromLine}-${reference.toLine}`;
      return `\`${reference.path}:${range}\`\n${fenced(cut(reference.text))}`;
    }
    case 'difference':
      return `\`${reference.path}\`\n${fenced(cut(reference.text), 'diff')}`;
    case 'output':
      return `${reference.command === null ? '' : `\`$ ${reference.command}\`\n`}${fenced(cut(reference.text))}`;
  }
}

/** Adds a reference to the message the person is writing to the thread's agent. */
export function addToMessage(threadId: string, reference: Reference): void {
  const before = useComposerStore.getState().text[threadId] ?? '';
  const text = referenceText(reference);
  composerWriter.setText(threadId, before.trim() === '' ? `${text}\n` : `${before.replace(/\s+$/, '')}\n\n${text}\n`);
}
