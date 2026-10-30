/** Unicode avatar text.
 *
 * An avatar shows one grapheme: a letter, a CJK character or an emoji, as a reader sees it and not as
 * it is stored. A flag, a family or a letter with its accents is one grapheme of several code points,
 * so the text is cut by `Intl.Segmenter` and never by index.
 */

/** How many colours the identity palette has. */
export const IDENTITY_COLORS = 12;

const segmenter = new Intl.Segmenter(undefined, { granularity: 'grapheme' });

/** The graphemes of a text, in order. */
export function graphemes(text: string): string[] {
  return Array.from(segmenter.segment(text), (part) => part.segment);
}

/** How many graphemes a text has: its length as a reader counts it. */
export function graphemeCount(text: string): number {
  let count = 0;
  for (const _ of segmenter.segment(text)) count += 1;
  return count;
}

/**
 * The text an avatar shows for a name: its first grapheme that is not white space, a letter in upper
 * case. An empty name gives an empty text, and the avatar then shows its shape alone.
 */
export function avatarText(name: string): string {
  for (const { segment } of segmenter.segment(name.trim())) {
    if (segment.trim() === '') continue;
    return segment.toLocaleUpperCase();
  }
  return '';
}

/**
 * A colour of the identity palette for something that was given none: always the same one for the
 * same identifier, and spread evenly over the palette (FNV-1a over its characters).
 */
export function identityColor(id: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < id.length; index += 1) {
    hash ^= id.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0) % IDENTITY_COLORS;
}
