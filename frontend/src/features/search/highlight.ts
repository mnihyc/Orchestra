/** Lighting words where they stand: the words a find looks for, marked in the text of whatever is drawn, without touching what draws it.
 *
 * The browser's own highlight registry takes ranges of text and paints them, so the conversation, which
 * is drawn by other code and a few rows at a time, is never changed: the ranges are found in the text that
 * is on the page and are made again as rows come and go. A browser without the registry shows nothing
 * lit, and the find still steps from hit to hit.
 */

/** The name the registry holds the lit words under, and the one the stylesheet paints. */
export const FIND_HIGHLIGHT = 'orch-find';

/** The most ranges lit at once: a word that is everywhere is lit where it can be read, not everywhere. */
const MOST_RANGES = 2_000;

// Several views can be searched together; each keeps its ranges until its own bar closes.
const litViews = new Map<string, Range[]>();
const MAIN_FIND = 'conversation';

interface Registry {
  set: (name: string, highlight: unknown) => void;
  delete: (name: string) => void;
}

/** The registry, where the browser has one. */
function registry(): Registry | null {
  const found = (globalThis as { CSS?: { highlights?: Registry } }).CSS?.highlights;
  return found === undefined || typeof (globalThis as { Highlight?: unknown }).Highlight !== 'function' ? null : found;
}

/** The ranges of `words` inside the text of an element, found without regard to case. */
export function rangesOf(root: Element, words: string): Range[] {
  const wanted = words.trim().toLowerCase();
  if (wanted === '') return [];
  const ranges: Range[] = [];
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node !== null && ranges.length < MOST_RANGES; node = walker.nextNode()) {
    const text = (node as Text).data.toLowerCase();
    for (let at = text.indexOf(wanted); at !== -1 && ranges.length < MOST_RANGES; at = text.indexOf(wanted, at + wanted.length)) {
      const range = document.createRange();
      range.setStart(node, at);
      range.setEnd(node, at + wanted.length);
      ranges.push(range);
    }
  }
  return ranges;
}

/** Lights the words in an element; with no words, or no element, lights nothing. */
export function lightWords(root: Element | null, words: string, owner = MAIN_FIND): void {
  const highlights = registry();
  if (highlights === null) return;
  const own = root === null ? [] : rangesOf(root, words);
  if (own.length === 0) litViews.delete(owner);
  else litViews.set(owner, own);
  const ranges = [...litViews.values()].flat().slice(0, MOST_RANGES);
  if (ranges.length === 0) {
    highlights.delete(FIND_HIGHLIGHT);
    return;
  }
  const Highlight = (globalThis as unknown as { Highlight: new (...ranges: Range[]) => unknown }).Highlight;
  highlights.set(FIND_HIGHLIGHT, new Highlight(...ranges));
}

export function clearLight(owner = MAIN_FIND): void {
  litViews.delete(owner);
  const highlights = registry();
  if (highlights === null) return;
  const ranges = [...litViews.values()].flat().slice(0, MOST_RANGES);
  if (ranges.length === 0) highlights.delete(FIND_HIGHLIGHT);
  else {
    const Highlight = (globalThis as unknown as { Highlight: new (...ranges: Range[]) => unknown }).Highlight;
    highlights.set(FIND_HIGHLIGHT, new Highlight(...ranges));
  }
}
