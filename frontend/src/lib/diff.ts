/** A unified difference read into hunks and lines, with the words that changed inside a changed line marked.
 *
 * The server records a change to a file as a unified diff with three lines of context. This reads it
 * into hunks, numbers every line on both sides, pairs the lines that were removed with the lines that
 * took their place, and finds, inside each pair, the runs of words that really changed, so that a
 * changed line is read at the words and not as two whole lines.
 */

export type LineKind = 'context' | 'add' | 'del';

export interface DiffLine {
  kind: LineKind;
  /** The text of the line, without the leading mark. */
  text: string;
  /** Its number in the old file; absent for an added line. */
  oldNo?: number;
  /** Its number in the new file; absent for a removed line. */
  newNo?: number;
  /** For a line that has a partner on the other side: the ranges of its text that changed, as `[from, to)` offsets. */
  marks?: readonly (readonly [number, number])[];
  /** The line after it has no newline at its end. */
  noNewline?: boolean;
}

export interface Hunk {
  oldStart: number;
  oldCount: number;
  newStart: number;
  newCount: number;
  /** What follows the numbers in the hunk's header: usually the function the change is in. */
  heading: string;
  lines: DiffLine[];
}

const HEADER = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@ ?(.*)$/;

/** The longest line whose words are compared: beyond it the comparison costs more than it shows. */
export const WORD_DIFF_MAX_CHARS = 2_000;
/** The most tokens of one line that are compared; a quadratic comparison is kept under a few million steps. */
const WORD_DIFF_MAX_TOKENS = 600;

/** Reads a unified diff. Anything before the first hunk (file headers) is skipped. */
export function parseDiff(text: string): Hunk[] {
  const hunks: Hunk[] = [];
  let current: Hunk | null = null;
  let oldNo = 0;
  let newNo = 0;
  for (const raw of text.split('\n')) {
    const header = HEADER.exec(raw);
    if (header !== null) {
      current = {
        oldStart: Number(header[1]),
        oldCount: header[2] === undefined ? 1 : Number(header[2]),
        newStart: Number(header[3]),
        newCount: header[4] === undefined ? 1 : Number(header[4]),
        heading: header[5] ?? '',
        lines: [],
      };
      hunks.push(current);
      oldNo = current.oldStart;
      newNo = current.newStart;
      continue;
    }
    if (current === null) continue;
    if (raw.startsWith('\\')) {
      // "\ No newline at end of file" belongs to the line before it.
      const last = current.lines.at(-1);
      if (last !== undefined) last.noNewline = true;
      continue;
    }
    const mark = raw.charAt(0);
    const rest = raw.slice(1);
    if (mark === '+') current.lines.push({ kind: 'add', text: rest, newNo: newNo++ });
    else if (mark === '-') current.lines.push({ kind: 'del', text: rest, oldNo: oldNo++ });
    else if (mark === ' ') current.lines.push({ kind: 'context', text: rest, oldNo: oldNo++, newNo: newNo++ });
    // The empty last line of the text, or a blank line the server did not pad: not part of the change.
    else if (raw === '' && current.lines.length > 0 && oldNo - current.oldStart + newNo - current.newStart < current.oldCount + current.newCount) {
      current.lines.push({ kind: 'context', text: '', oldNo: oldNo++, newNo: newNo++ });
    }
  }
  for (const hunk of hunks) markWords(hunk);
  return hunks;
}

/** Splits a line into words, runs of spaces and single marks, so that a change of one word is one token. */
export function tokens(line: string): string[] {
  return line.match(/\s+|[\p{L}\p{N}_]+|[^\s\p{L}\p{N}_]/gu) ?? [];
}

const isSpace = (part: string): boolean => /^\s+$/.test(part);

/** The longest common subsequence of two token lists, as the pairs of indexes that match. */
function commonRuns(one: readonly string[], other: readonly string[]): [number, number][] {
  const rows = one.length + 1;
  const cols = other.length + 1;
  const table = new Uint16Array(rows * cols);
  for (let i = one.length - 1; i >= 0; i -= 1) {
    for (let j = other.length - 1; j >= 0; j -= 1) {
      table[i * cols + j] =
        one[i] === other[j]
          ? (table[(i + 1) * cols + j + 1] ?? 0) + 1
          : Math.max(table[(i + 1) * cols + j] ?? 0, table[i * cols + j + 1] ?? 0);
    }
  }
  const pairs: [number, number][] = [];
  let i = 0;
  let j = 0;
  while (i < one.length && j < other.length) {
    if (one[i] === other[j]) {
      pairs.push([i, j]);
      i += 1;
      j += 1;
    } else if ((table[(i + 1) * cols + j] ?? 0) >= (table[i * cols + j + 1] ?? 0)) i += 1;
    else j += 1;
  }
  return pairs;
}

/**
 * The ranges of a line (as character offsets) whose tokens are not among the matched ones. A run of
 * changed tokens is one range, without the spaces at its ends: a space that alone changed is not marked.
 */
function changedRanges(parts: readonly string[], line: string, matched: ReadonlySet<number>): [number, number][] {
  const runs: [number, number][] = [];
  let at = 0;
  parts.forEach((part, index) => {
    const end = at + part.length;
    if (!matched.has(index)) {
      const last = runs.at(-1);
      if (last?.[1] === at) last[1] = end;
      else runs.push([at, end]);
    }
    at = end;
  });
  const ranges: [number, number][] = [];
  for (const [from, to] of runs) {
    const text = line.slice(from, to);
    const start = from + (text.length - text.trimStart().length);
    const finish = to - (text.length - text.trimEnd().length);
    if (finish > start) ranges.push([start, finish]);
  }
  return ranges;
}

/** Marks, in each run of removed lines followed by added lines, the words that changed between the lines that pair up. */
function markWords(hunk: Hunk): void {
  const lines = hunk.lines;
  let at = 0;
  while (at < lines.length) {
    if (lines[at]?.kind !== 'del') {
      at += 1;
      continue;
    }
    let delEnd = at;
    while (lines[delEnd]?.kind === 'del') delEnd += 1;
    let addEnd = delEnd;
    while (lines[addEnd]?.kind === 'add') addEnd += 1;
    const pairs = Math.min(delEnd - at, addEnd - delEnd);
    for (let k = 0; k < pairs; k += 1) {
      const removed = lines[at + k];
      const added = lines[delEnd + k];
      if (removed === undefined || added === undefined) continue;
      if (removed.text.length > WORD_DIFF_MAX_CHARS || added.text.length > WORD_DIFF_MAX_CHARS) continue;
      const left = tokens(removed.text);
      const right = tokens(added.text);
      if (left.length > WORD_DIFF_MAX_TOKENS || right.length > WORD_DIFF_MAX_TOKENS) continue;
      const matches = commonRuns(left, right);
      // A line that shares almost nothing with its partner is a new line, not an edited one: the spaces between words are shared by any two lines of words, so only the rest counts.
      const shared = matches.filter(([at]) => !isSpace(left[at] ?? '')).length;
      const fewest = Math.min(left.filter((part) => !isSpace(part)).length, right.filter((part) => !isSpace(part)).length);
      if (shared === 0 || shared * 4 < fewest) continue;
      removed.marks = changedRanges(left, removed.text, new Set(matches.map(([i]) => i)));
      added.marks = changedRanges(right, added.text, new Set(matches.map(([, j]) => j)));
    }
    at = addEnd;
  }
}

/** How many lines of old and of new text lie between the end of one hunk and the start of the next. */
export function unchangedBetween(before: Hunk, after: Hunk): number {
  return Math.max(0, after.oldStart - (before.oldStart + before.oldCount));
}

/** The two sides of a hunk laid side by side: a removed line faces the added line that took its place. */
export interface SideRow {
  left: DiffLine | null;
  right: DiffLine | null;
}

export function sideBySide(hunk: Hunk): SideRow[] {
  const rows: SideRow[] = [];
  const lines = hunk.lines;
  let at = 0;
  while (at < lines.length) {
    const line = lines[at];
    if (line === undefined) break;
    if (line.kind === 'context') {
      rows.push({ left: line, right: line });
      at += 1;
      continue;
    }
    let delEnd = at;
    while (lines[delEnd]?.kind === 'del') delEnd += 1;
    let addEnd = delEnd;
    while (lines[addEnd]?.kind === 'add') addEnd += 1;
    const height = Math.max(delEnd - at, addEnd - delEnd);
    for (let k = 0; k < height; k += 1) rows.push({ left: at + k < delEnd ? (lines[at + k] ?? null) : null, right: delEnd + k < addEnd ? (lines[delEnd + k] ?? null) : null });
    at = addEnd;
  }
  return rows;
}

/** Counts the lines a diff adds and removes. */
export function countChanges(hunks: readonly Hunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      if (line.kind === 'add') added += 1;
      else if (line.kind === 'del') removed += 1;
    }
  }
  return { added, removed };
}
