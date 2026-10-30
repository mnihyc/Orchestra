/** Safe HTML-to-block conversion: the one converter the server's importer runs, as the browser tests and the fake server have it, with explicit conversion losses. */
import { readHtml as convert } from '../../../../shared/document-html.js';
import type { Piece } from '../documents/pieces.ts';

export interface ConvertedHtml { pieces: Piece[]; warnings: string[]; images: { id: string; src: string; alt: string }[] }

/** The parser has no DOM, script or network access in either runtime. */
export function readHtml(source: string, _parse?: (source: string) => globalThis.Document): ConvertedHtml {
  return convert(source);
}
