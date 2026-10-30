/** Waiting-input text edits, stable reorder targets and the delivery of an explicit Send now. */
import type { ContentPart, Delivery } from '@/generated/api-types';

/** Only the text the author can edit here; forwarded quotes and attachments keep their own parts. */
export function editableWords(content: readonly ContentPart[]): string {
  return content.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n');
}

/** A meaningful row for text, an attachment-only input or an untouched forwarded quote. */
export function queuePreview(content: readonly ContentPart[]): string {
  for (const part of content) {
    if (part.type !== 'text' && part.type !== 'forwarded') continue;
    // The scripted agent's delimiter carries no words of the message. A real shebang,
    // Markdown heading or command is still meaningful and must keep its exact text.
    const line = part.text.split(/\r?\n/).map((words) => words.trim()).find((words) => words !== '' && words !== '#!fake');
    if (line !== undefined) return line;
  }
  return content.flatMap((part) => part.type === 'image' || part.type === 'file' ? [part.name] : part.type === 'forwarded' ? [...part.images, ...part.attachments].map((file) => file.name) : []).join(', ');
}

/** Replaces plain text at its first position and leaves every other part, and its order, intact. */
export function withEditedWords(content: readonly ContentPart[], words: string): ContentPart[] {
  const text: ContentPart[] = words.trim() === '' ? [] : [{ type: 'text', text: words }];
  let placed = false;
  const parts: ContentPart[] = [];
  for (const part of content) {
    if (part.type !== 'text') parts.push(part);
    else if (!placed) { parts.push(...text); placed = true; }
  }
  if (!placed) parts.unshift(...text);
  return parts;
}

/** The predecessor after moving an existing row to an index; undefined means no change or a stale row. */
export function afterMoving(ids: readonly string[], id: string, index: number): string | null | undefined {
  const from = ids.indexOf(id);
  if (from < 0 || index < 0 || index >= ids.length || index === from) return undefined;
  const others = ids.filter((one) => one !== id);
  return index === 0 ? null : others[index - 1];
}

/** A line belongs to a target input, not its changing row number; resolve it against the latest queue. */
export function afterDropping(ids: readonly string[], id: string, target: string, edge: 'before' | 'after'): string | null | undefined {
  if (id === target || !ids.includes(id)) return undefined;
  const others = ids.filter((one) => one !== id);
  const index = others.indexOf(target);
  return index < 0 ? undefined : afterMoving(ids, id, index + (edge === 'after' ? 1 : 0));
}

/** No turn: first in line. A running turn: steer, or interrupt when this agent has refused steering. */
export function nowDelivery(active: boolean, cannotSteer: boolean): Delivery {
  return active ? cannotSteer ? 'interrupt' : 'steer' : 'queue';
}
