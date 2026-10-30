/** How the finder ranks what it holds itself: settings, rows of a page, commands and help, for the letters typed so far.
 *
 * Every word typed has to be found somewhere in an entry (in any order), a title counts for more than the
 * words that were added to find it by, and those for more than the sentence that explains it. A word that
 * begins the title beats one that begins a word of it, and that beats one found inside a word, which
 * is the only way to find something in a script that has no spaces. Case and accents are no difference.
 * It runs on every key, so everything an entry is matched by is made once, when the entry is prepared.
 */

/** One entry as the finder ranks it: what it is called, the words it can be found by, and the sentence that says what it is. */
export interface Searchable {
  title: string;
  /** Words that are no part of the title but find the entry: a key, a synonym. */
  words?: readonly string[];
  /** The sentence that explains it. */
  detail?: string;
}

/** An entry made ready to be matched: folded once, so that a key press costs a few searches in short strings. */
export interface Prepared {
  title: string;
  words: string;
  detail: string;
}

/** What a word found in each part of an entry is worth. A title counts most, since it is what a person means to type. */
const TITLE_WEIGHT = 1;
const WORDS_WEIGHT = 0.7;
const DETAIL_WEIGHT = 0.4;

/** What a word is worth by where it was found in a part: a whole word, the beginning of the part, the beginning of a word, or inside one. */
const WHOLE_WORD = 1;
const BEGINS_PART = 0.95;
const WHOLE_WORD_LATER = 0.9;
const BEGINS_WORD = 0.85;
const INSIDE = 0.6;

/** Auxiliary wording breaks ties inside a name tier, without overtaking that tier. */
const DETAIL_TIE_WEIGHT = 0.2;

/** One character folded to its plain, lower-case form, in as many UTF-16 units as it had, so that places in the folded text are places in the original. */
function foldCharacter(character: string): string {
  // The first code point of the decomposed form is the letter without its accents.
  const letter = Array.from(character.normalize('NFD'))[0] ?? character;
  const lower = letter.toLowerCase();
  if (lower.length === character.length) return lower;
  // A character whose plain form is longer (a ligature) is folded by its compatibility form, or left as it is.
  const compatible = character.normalize('NFKC').toLowerCase();
  return compatible.length === character.length ? compatible : character;
}

/** Text without case or accents, the same length as it was. */
export function fold(text: string): string {
  let out = '';
  for (const character of text) out += foldCharacter(character);
  return out;
}

/** Makes an entry ready to be ranked. */
export function prepare(entry: Searchable): Prepared {
  return { title: fold(entry.title), words: fold((entry.words ?? []).join(' ')), detail: fold(entry.detail ?? '') };
}

/** Every word is required; double quotes keep a phrase together, even while its closing quote is not typed yet. */
export function wordsOf(query: string): string[] {
  const terms: string[] = [];
  let term = '';
  let quoted = false;
  const finish = () => { if (term.trim() !== '') terms.push(term.trim()); term = ''; };
  for (const character of fold(query)) {
    if (character === '"') quoted = !quoted;
    else if (/\s/u.test(character) && !quoted) finish();
    else term += character;
  }
  finish();
  return terms;
}

/** The literal matcher shared by the finder, resource lists and tree filter. */
export function matches(query: string, ...fields: readonly (string | null | undefined)[]): boolean {
  const text = fold(fields.filter((field): field is string => typeof field === 'string').join('\n'));
  return wordsOf(query).every((term) => text.includes(term));
}

const LETTER_OR_DIGIT = /[\p{L}\p{N}]/u;

/** Where `word` first appears in `haystack` at the beginning of a word, or -1. */
function wordStart(haystack: string, word: string): number {
  for (let at = haystack.indexOf(word); at !== -1; at = haystack.indexOf(word, at + 1)) {
    if (at === 0 || !LETTER_OR_DIGIT.test(haystack.charAt(at - 1))) return at;
  }
  return -1;
}

/** Whether `word` is a whole word of `haystack`: a word at its beginning that ends where a word ends. */
function isWholeWord(haystack: string, word: string): boolean {
  for (let at = haystack.indexOf(word); at !== -1; at = haystack.indexOf(word, at + 1)) {
    const before = at === 0 || !LETTER_OR_DIGIT.test(haystack.charAt(at - 1));
    const end = at + word.length;
    const after = end >= haystack.length || !LETTER_OR_DIGIT.test(haystack.charAt(end));
    if (before && after) return true;
  }
  return false;
}

/** What `word` is worth in one part of an entry; 0 when it is not in it. */
function worth(haystack: string, word: string): number {
  if (haystack === '' || !haystack.includes(word)) return 0;
  const begins = haystack.startsWith(word);
  if (isWholeWord(haystack, word)) return begins ? WHOLE_WORD : WHOLE_WORD_LATER;
  if (begins) return BEGINS_PART;
  if (wordStart(haystack, word) !== -1) return BEGINS_WORD;
  return INSIDE;
}

/**
 * How well an entry answers the words typed, or `null` when some word is nowhere in it. The score
 * is the average of what each word is worth in its best part, and the whole query being the title,
 * or beginning it, adds a little.
 */
export function score(words: readonly string[], entry: Prepared): number | null {
  if (words.length === 0) return 0;
  let total = 0;
  for (const word of words) {
    const best = Math.max(worth(entry.title, word) * TITLE_WEIGHT, worth(entry.words, word) * WORDS_WEIGHT, worth(entry.detail, word) * DETAIL_WEIGHT);
    if (best === 0) return null;
    total += best;
  }
  return nameTier(entry.title, words) + (total / words.length) * DETAIL_TIE_WEIGHT;
}

export interface Ranked<Entry> {
  entry: Entry;
  score: number;
}

/**
 * The entries that answer the query, best first; entries that tie keep the order they were given in, so
 * that a list that is ranked again after one more letter does not shuffle. With nothing typed, all of them in their order.
 */
export function rank<Entry>(entries: readonly Entry[], prepared: (entry: Entry) => Prepared, query: string, limit: number = Number.POSITIVE_INFINITY): Ranked<Entry>[] {
  const words = wordsOf(query);
  if (words.length === 0) return entries.slice(0, limit).map((entry) => ({ entry, score: 0 }));
  const found: (Ranked<Entry> & { index: number })[] = [];
  entries.forEach((entry, index) => {
    const result = score(words, prepared(entry));
    if (result !== null) found.push({ entry, score: result, index });
  });
  found.sort((one, other) => other.score - one.score || one.index - other.index);
  return found.slice(0, limit).map(({ entry, score: points }) => ({ entry, score: points }));
}

/** The ranges of `text` that the words typed fall on, as [start, end) in its UTF-16 units: what a result marks. */
export function markedRanges(text: string, query: string): [number, number][] {
  const folded = fold(text);
  const ranges: [number, number][] = [];
  for (const word of wordsOf(query)) {
    for (let at = folded.indexOf(word); at !== -1; at = folded.indexOf(word, at + word.length)) ranges.push([at, at + word.length]);
  }
  ranges.sort((one, other) => one[0] - other[0] || other[1] - one[1]);
  // Overlapping and touching ranges are one mark.
  const merged: [number, number][] = [];
  for (const range of ranges) {
    const last = merged.at(-1);
    if (last !== undefined && range[0] <= last[1]) last[1] = Math.max(last[1], range[1]);
    else merged.push([range[0], range[1]]);
  }
  return merged;
}

/**
 * How well a name that the server found answers the letters typed: the name itself, its beginning, a word of
 * it, the inside of it, and last what the server matched by something that is not the name (an address, a
 * username). The server decides what is found; this puts the groups of names in order beside the groups the
 * page holds, which are ranked by `score`.
 */
export function matchScore(name: string, query: string): number {
  return nameTier(fold(name), wordsOf(query));
}

function nameTier(name: string, terms: readonly string[]): number {
  if (terms.length === 0 || !terms.every((term) => name.includes(term))) return NAME_NOT_MATCHED;
  const phrase = terms.join(' ');
  if (name === phrase) return NAME_IS_QUERY;
  if (name.startsWith(phrase)) return NAME_BEGINS;
  if (terms.every((term) => wordStart(name, term) !== -1)) return WORD_BEGINS;
  return NAME_HAS;
}

/** What a name found by the server is worth by how it answers: above a title of the page that was named, and in the same order of tiers. */
const NAME_IS_QUERY = 4;
const NAME_BEGINS = 3;
const WORD_BEGINS = 2;
const NAME_HAS = 1;
const NAME_NOT_MATCHED = 0;
