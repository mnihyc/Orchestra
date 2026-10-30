/** A small stand-in for the server's reader of Markdown, for the fake server of the browser tests: what is pasted as Markdown becomes the blocks it says, and no more is guessed than Markdown itself says. The real reader is the server's (`orch_documents::convert`); the page has none.
 *
 * The reader is small and has no dependency: headings, lists with their nesting and their to-dos, quotes
 * (and the alerts GitHub writes as quotes, which are callouts), fenced code, tables, dividers and
 * paragraphs, with bold, italic, strike-through, code, links and formulas inside a line. What a block
 * cannot hold is kept as the words it was written with: an image is its description as a link, raw HTML
 * is text. Nothing here knows an editor; `paste.ts` lays the result on the page.
 */
import type { BlockType, Props } from '../../src/features/documents/blocks.ts';
import { linkAllowed } from '../../src/features/people/links.ts';
import type { Inline, Mark, TableRow } from '../../src/generated/api-types.ts';

/** A block of what was read, with the blocks under it. */
export interface Piece {
  type: BlockType;
  props: Props;
  content: Inline[] | TableRow[];
  children: Piece[];
}

type Run = Extract<Inline, { type: 'text' }>;

const piece = (type: BlockType, props: Props, content: Inline[] | TableRow[] = [], children: Piece[] = []): Piece => ({ type, props, content, children });

// ---------------------------------------------------------------- inside a line

/** Whether a character is one a backslash may escape. */
const ESCAPABLE = /[\\`*_{}[\]()#+\-.!|~<>$"']/;

/** The end of the `[` text that starts at `from`, matching brackets: the index of its `]`, or -1. */
function closingBracket(text: string, from: number): number {
  let depth = 0;
  for (let at = from; at < text.length; at += 1) {
    const char = text[at];
    if (char === '\\') at += 1;
    else if (char === '[') depth += 1;
    else if (char === ']') {
      depth -= 1;
      if (depth === 0) return at;
    }
  }
  return -1;
}

/** The address of a link whose `(` is at `open`: its text up to the first space, and where the `)` is. */
function linkTarget(text: string, open: number): { address: string; end: number } | null {
  let depth = 0;
  for (let at = open; at < text.length; at += 1) {
    const char = text[at];
    if (char === '\\') at += 1;
    else if (char === '(') depth += 1;
    else if (char === ')') {
      depth -= 1;
      if (depth === 0) {
        // The part after a space is a title, which a link here does not keep.
        const inside = text.slice(open + 1, at).trim();
        const address = inside.startsWith('<') && inside.includes('>') ? inside.slice(1, inside.indexOf('>')) : (inside.split(/\s+/)[0] ?? '');
        return { address, end: at };
      }
    }
  }
  return null;
}

/** Whether an emphasis may open or close at an index: not beside a space, and a `_` not inside a word. */
function flanks(text: string, at: number, length: number, closing: boolean): boolean {
  const before = text[at - 1] ?? ' ';
  const after = text[at + length] ?? ' ';
  const char = text[at] ?? '';
  const side = closing ? before : after;
  if (/\s/.test(side)) return false;
  if (char === '_') {
    const outside = closing ? after : before;
    if (/[\p{L}\p{N}]/u.test(outside)) return false;
  }
  return true;
}

/** Reads the marks and links of a line into runs of text. */
export function readInline(text: string, marks: readonly Mark[] = [], href: string | null = null): Inline[] {
  const out: Inline[] = [];
  let word = '';
  const flush = () => {
    if (word === '') return;
    const run: Run = { type: 'text', text: word };
    if (marks.length > 0) run.marks = [...marks];
    if (href !== null) run.href = href;
    out.push(run);
    word = '';
  };
  const push = (inner: Inline[]) => {
    flush();
    out.push(...inner);
  };
  let at = 0;
  while (at < text.length) {
    const char = text[at] ?? '';
    const escaped = text[at + 1] ?? '';
    if (char === '\\' && escaped !== '' && ESCAPABLE.test(escaped)) {
      word += escaped;
      at += 2;
      continue;
    }
    if (char === '`') {
      let ticks = 1;
      while (text[at + ticks] === '`') ticks += 1;
      const fence = '`'.repeat(ticks);
      const end = text.indexOf(fence, at + ticks);
      if (end > at + ticks) {
        flush();
        const run: Run = { type: 'text', text: text.slice(at + ticks, end).replace(/^ (.*) $/, '$1'), marks: ['code'] };
        if (href !== null) run.href = href;
        out.push(run);
        at = end + ticks;
        continue;
      }
      word += fence;
      at += ticks;
      continue;
    }
    if (char === '!' && text[at + 1] === '[') {
      // An image cannot be kept in a line: its description stays, as a link to where it was.
      const close = closingBracket(text, at + 1);
      const target = close > 0 && text[close + 1] === '(' ? linkTarget(text, close + 1) : null;
      if (close > 0 && target !== null) {
        const description = text.slice(at + 2, close);
        const label = description === '' ? target.address : description;
        push(linkAllowed(target.address) ? readInline(label, marks, target.address) : readInline(label, marks, href));
        at = target.end + 1;
        continue;
      }
    }
    if (char === '[') {
      const close = closingBracket(text, at);
      const target = close > 0 && text[close + 1] === '(' ? linkTarget(text, close + 1) : null;
      if (close > 0 && target !== null) {
        const inner = text.slice(at + 1, close);
        push(linkAllowed(target.address) ? readInline(inner, marks, target.address) : readInline(inner, marks, href));
        at = target.end + 1;
        continue;
      }
    }
    if (char === '<') {
      const found = /^<((?:https?:\/\/|mailto:)[^\s<>]+)>/i.exec(text.slice(at));
      if (found?.[1] !== undefined) {
        push(readInline(found[1], marks, found[1]));
        at += found[0].length;
        continue;
      }
    }
    if (char === '$') {
      // A formula in a line: no space inside the dollar signs, and no digit straight after the second.
      const close = text.indexOf('$', at + 1);
      const source = close > at + 1 ? text.slice(at + 1, close) : '';
      if (source !== '' && !/^\s|\s$/.test(source) && !/\d/.test(text[close + 1] ?? '')) {
        push([{ type: 'math', source }]);
        at = close + 1;
        continue;
      }
    }
    if (char === '*' || char === '_' || char === '~') {
      let length = 1;
      while (text[at + length] === char) length += 1;
      const tilde = char === '~';
      const size = tilde ? (length >= 2 ? 2 : 0) : Math.min(length, 3);
      if (size > 0 && flanks(text, at, size, false)) {
        const fence = char.repeat(size);
        // The nearest closing run of the same size that may close.
        let search = at + size;
        let end = -1;
        while (search < text.length) {
          const found = text.indexOf(fence, search);
          if (found < 0) break;
          let run = size;
          while (text[found + run] === char) run += 1;
          if (run === size && flanks(text, found, size, true) && found > at + size) {
            end = found;
            break;
          }
          search = found + run;
        }
        if (end > 0) {
          const added: Mark[] = tilde ? ['strike'] : size === 3 ? ['bold', 'italic'] : size === 2 ? ['bold'] : ['italic'];
          push(readInline(text.slice(at + size, end), [...marks.filter((mark) => !added.includes(mark)), ...added], href));
          at = end + size;
          continue;
        }
      }
      word += char.repeat(length);
      at += length;
      continue;
    }
    word += char;
    at += 1;
  }
  flush();
  return out;
}

/** What stands for a line that ends for good, while a paragraph is read as one line. */
const HARD = '\u0001';

/** The lines of a paragraph as one line of runs: a soft break is a space, and two spaces or a backslash end a line for good. */
function readLines(lines: readonly string[]): Inline[] {
  const joined = lines
    .map((line, index) => {
      const hard = / {2,}$/.test(line) || line.endsWith('\\');
      const body = line.replace(/(?: {2,}|\\)$/, '').trim();
      return index < lines.length - 1 ? body + (hard ? HARD : ' ') : body;
    })
    .join('');
  const out: Inline[] = [];
  for (const run of readInline(joined)) {
    if (run.type !== 'text' || !run.text.includes(HARD)) {
      out.push(run);
      continue;
    }
    run.text.split(HARD).forEach((part, index) => {
      if (index > 0) out.push({ type: 'break' });
      if (part !== '') out.push({ ...run, text: part });
    });
  }
  return merge(out);
}

/** Joins neighbouring runs that say the same of their words. */
function merge(runs: Inline[]): Inline[] {
  const out: Inline[] = [];
  for (const run of runs) {
    const last = out[out.length - 1];
    if (
      last?.type === 'text' &&
      run.type === 'text' &&
      (last.href ?? null) === (run.href ?? null) &&
      JSON.stringify(last.marks ?? []) === JSON.stringify(run.marks ?? [])
    ) {
      last.text += run.text;
    } else {
      out.push(run.type === 'text' ? { ...run } : run);
    }
  }
  return out.filter((run) => run.type !== 'text' || run.text !== '');
}

// ---------------------------------------------------------------- blocks

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^\s`]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})(?:\s+(.*?))?(?:\s+#+)?\s*$/;
const DIVIDER = /^ {0,3}([-*_])(?:\s*\1){2,}\s*$/;
const ITEM = /^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$/;
const QUOTE = /^ {0,3}>\s?(.*)$/;
const DELIMITER = /^\s*\|?\s*:?-+:?\s*(\|\s*:?-+:?\s*)*\|?\s*$/;
const ALERTS: Readonly<Record<string, string>> = { NOTE: 'info', TIP: 'success', IMPORTANT: 'info', WARNING: 'warning', CAUTION: 'danger' };

/** The cells of a table's line. */
function cellsOf(line: string): string[] {
  const body = line.trim().replace(/^\|/, '').replace(/\|$/, '');
  const cells: string[] = [];
  let cell = '';
  for (let at = 0; at < body.length; at += 1) {
    const char = body[at] ?? '';
    if (char === '\\' && body[at + 1] === '|') {
      cell += '|';
      at += 1;
    } else if (char === '|') {
      cells.push(cell.trim());
      cell = '';
    } else {
      cell += char;
    }
  }
  cells.push(cell.trim());
  return cells;
}

/** Whether a line begins something other than a paragraph. */
function startsBlock(line: string, next: string | undefined): boolean {
  if (FENCE.test(line) || HEADING.test(line) || QUOTE.test(line) || DIVIDER.test(line) || ITEM.test(line)) return true;
  return line.includes('|') && next !== undefined && next.includes('-') && DELIMITER.test(next);
}

/** Reads Markdown into blocks, in the order it is written. */
export function readMarkdown(source: string): Piece[] {
  const lines = source.replace(/\r\n?/g, '\n').replace(/\t/g, '    ').split('\n');
  const out: Piece[] = [];
  // The items of the list being read, from the outermost: where each one's marker stood and the piece it made.
  let stack: { indent: number; piece: Piece }[] = [];
  let at = 0;
  const add = (made: Piece) => {
    stack = [];
    out.push(made);
  };

  while (at < lines.length) {
    const start = at;
    const line = lines[at] ?? '';
    const next = lines[at + 1];
    if (line.trim() === '') {
      // A list goes on over a blank line when another item follows it.
      const following = lines.slice(at + 1).find((one) => one.trim() !== '');
      if (following === undefined || !ITEM.test(following)) stack = [];
      at += 1;
      continue;
    }

    const fence = FENCE.exec(line);
    if (fence !== null) {
      const marker = fence[1] ?? '```';
      const body: string[] = [];
      at += 1;
      while (at < lines.length && !new RegExp(`^ {0,3}${marker.startsWith('`') ? '`' : '~'}{${marker.length},}\\s*$`).test(lines[at] ?? '')) {
        body.push(lines[at] ?? '');
        at += 1;
      }
      at += 1;
      const code = body.join('\n');
      add(piece('code', { language: fence[2] ?? '' }, code === '' ? [] : [{ type: 'text', text: code }]));
      continue;
    }

    const heading = HEADING.exec(line);
    if (heading !== null) {
      add(piece('heading', { level: (heading[1] ?? '#').length }, readInline(heading[2] ?? '')));
      at += 1;
      continue;
    }

    if (DIVIDER.test(line)) {
      add(piece('divider', {}));
      at += 1;
      continue;
    }

    if (line.includes('|') && next !== undefined && next.includes('-') && DELIMITER.test(next)) {
      const head = cellsOf(line);
      const rows: TableRow[] = [{ cells: head.map((cell) => ({ content: readInline(cell) })) }];
      at += 2;
      while (at < lines.length && (lines[at] ?? '').trim() !== '' && (lines[at] ?? '').includes('|')) {
        const cells = cellsOf(lines[at] ?? '');
        rows.push({ cells: head.map((_cell, column) => ({ content: readInline(cells[column] ?? '') })) });
        at += 1;
      }
      add(piece('table', { header_row: true }, rows));
      continue;
    }

    if (QUOTE.test(line)) {
      const inside: string[] = [];
      while (at < lines.length && QUOTE.test(lines[at] ?? '')) {
        inside.push(QUOTE.exec(lines[at] ?? '')?.[1] ?? '');
        at += 1;
      }
      const alert = /^\[!(NOTE|TIP|IMPORTANT|WARNING|CAUTION)\]\s*$/i.exec(inside[0] ?? '');
      const rest = alert === null ? inside : inside.slice(1);
      const content = readLines(rest.filter((one, index) => !(one.trim() === '' && index === 0)));
      add(alert === null ? piece('quote', {}, content) : piece('callout', { tone: ALERTS[(alert[1] ?? 'NOTE').toUpperCase()] ?? 'info' }, content));
      continue;
    }

    const item = ITEM.exec(line);
    if (item !== null) {
      const indent = (item[1] ?? '').length;
      const marker = item[2] ?? '-';
      const ordered = /\d/.test(marker);
      const body = (item[3] ?? '').trimEnd();
      const box = ordered ? null : /^\[([ xX])\]\s+(.*)$/.exec(body);
      at += 1;
      // Lines under an item that are indented past its marker go on in it.
      const more: string[] = [];
      while (
        at < lines.length &&
        (lines[at] ?? '').trim() !== '' &&
        !startsBlock(lines[at] ?? '', lines[at + 1]) &&
        (lines[at]?.search(/\S/) ?? 0) > indent
      ) {
        more.push((lines[at] ?? '').trim());
        at += 1;
      }
      const words = readLines([box === null ? body : (box[2] ?? ''), ...more]);
      const made = box !== null ? piece('todo', { checked: box[1] !== ' ' }, words) : ordered ? piece('numbered', {}, words) : piece('bullet', {}, words);
      while (stack.length > 0 && (stack[stack.length - 1]?.indent ?? 0) >= indent) stack.pop();
      const parent = stack[stack.length - 1]?.piece;
      const siblings = parent === undefined ? out : parent.children;
      if (ordered) {
        const number = Number.parseInt(marker, 10);
        // A run of numbers says where it starts, once, when it is not at one.
        if (siblings[siblings.length - 1]?.type !== 'numbered' && number !== 1) made.props = { start: number };
      }
      siblings.push(made);
      stack.push({ indent, piece: made });
      continue;
    }

    // A paragraph runs to a blank line or to whatever starts another block; a line of equals signs or dashes under it makes it a heading.
    const words: string[] = [];
    while (at < lines.length && (lines[at] ?? '').trim() !== '') {
      const current = lines[at] ?? '';
      if (words.length > 0 && startsBlock(current, lines[at + 1])) {
        if (DIVIDER.test(current) && /^ {0,3}-+\s*$/.test(current)) {
          // CommonMark: dashes under a line of text make it a second-level heading.
          add(piece('heading', { level: 2 }, readLines(words)));
          words.length = 0;
          at += 1;
        }
        break;
      }
      if (words.length > 0 && /^ {0,3}=+\s*$/.test(current)) {
        add(piece('heading', { level: 1 }, readLines(words)));
        words.length = 0;
        at += 1;
        break;
      }
      words.push(current);
      at += 1;
    }
    if (words.length > 0) add(piece('paragraph', {}, readLines(words)));
    // Whatever was met, the reading goes on: a line no branch took is skipped.
    else if (at === start) at += 1;
  }
  return out;
}

// ---------------------------------------------------------------- is it Markdown

/** Whether pasted text says something in Markdown that plain text does not: a block of one of its kinds, or a bold word, a link or code in a line. */
export function looksLikeMarkdown(text: string): boolean {
  const lines = text.replace(/\r\n?/g, '\n').split('\n');
  for (const [index, line] of lines.entries()) {
    if (FENCE.test(line) || /^ {0,3}#{1,6}\s+\S/.test(line) || QUOTE.test(line) || /^\s*([-*+]|\d{1,9}[.)])\s+\S/.test(line)) return true;
    if (DIVIDER.test(line) && index > 0) return true;
    if (line.includes('|') && DELIMITER.test(lines[index + 1] ?? '') && (lines[index + 1] ?? '').includes('-')) return true;
  }
  return /\*\*[^\s*][^*]*\*\*|`[^`\n]+`|\[[^\]\n]+\]\((?:https?:|mailto:|\/)[^)\s]+\)/.test(text);
}
