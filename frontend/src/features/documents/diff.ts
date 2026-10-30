/** The words that differ between two texts, for a difference that marks the changed words inside a block. */

export interface Piece {
  text: string;
  kind: 'same' | 'added' | 'removed';
}

/** The most words of either text that are compared word by word; beyond it the whole text is taken as replaced. */
const MOST_WORDS = 400;

function tokens(text: string): string[] {
  return text.match(/\s+|\S+/g) ?? [];
}

/** The pieces that turn `before` into `after`: what both have, what only `after` has, and what only `before` had. */
export function diffWords(before: string, after: string): Piece[] {
  const a = tokens(before);
  const b = tokens(after);
  if (a.length > MOST_WORDS || b.length > MOST_WORDS) {
    return [
      ...(before === '' ? [] : [{ text: before, kind: 'removed' as const }]),
      ...(after === '' ? [] : [{ text: after, kind: 'added' as const }]),
    ];
  }
  // The longest common run of words, by the usual table.
  const table: number[][] = Array.from({ length: a.length + 1 }, () => new Array<number>(b.length + 1).fill(0));
  for (let i = a.length - 1; i >= 0; i -= 1) {
    for (let j = b.length - 1; j >= 0; j -= 1) {
      const row = table[i];
      if (row === undefined) continue;
      row[j] = a[i] === b[j] ? (table[i + 1]?.[j + 1] ?? 0) + 1 : Math.max(table[i + 1]?.[j] ?? 0, row[j + 1] ?? 0);
    }
  }
  const pieces: Piece[] = [];
  const push = (kind: Piece['kind'], text: string) => {
    const last = pieces[pieces.length - 1];
    if (last?.kind === kind) last.text += text;
    else pieces.push({ text, kind });
  };
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      push('same', a[i] ?? '');
      i += 1;
      j += 1;
    } else if (j < b.length && (i === a.length || (table[i]?.[j + 1] ?? 0) >= (table[i + 1]?.[j] ?? 0))) {
      push('added', b[j] ?? '');
      j += 1;
    } else {
      push('removed', a[i] ?? '');
      i += 1;
    }
  }
  return pieces;
}
