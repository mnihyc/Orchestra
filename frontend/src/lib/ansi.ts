/** ANSI output rendered as content, never a terminal.
 *
 * What a command printed is read as text with its colours, not run through a terminal: only the
 * sequences that set how text looks (colour, weight, underline, reverse) are understood, every other
 * escape (moving the cursor, clearing the screen, a title) is dropped, and a carriage return that
 * rewrites a line (a progress bar) leaves the last version of that line. Nothing here emits markup:
 * the result is spans of plain text with a style, drawn by `ui/AnsiText`.
 */

/** How a run of text is drawn. A colour is an index of the 16 named ones, a step of the 256-colour cube, or a true colour. */
export interface AnsiStyle {
  fg?: AnsiColor;
  bg?: AnsiColor;
  bold?: true;
  dim?: true;
  italic?: true;
  underline?: true;
  inverse?: true;
}

export type AnsiColor = { kind: 'named'; index: number } | { kind: 'rgb'; r: number; g: number; b: number };

export interface AnsiSpan {
  text: string;
  style: AnsiStyle;
}

const ESC = '\u001b';
/** A control sequence: `ESC [`, parameters, one final byte. */
const CSI = new RegExp(`${ESC}\\[([0-9;:?]*)([ -/]*)([@-~])`, 'g');
/** The other escapes that carry text and must go whole: operating-system commands end with BEL or `ESC \`. */
const OSC = new RegExp(`${ESC}\\][^\\u0007${ESC}]*(?:\\u0007|${ESC}\\\\)`, 'g');
/** Any other two-byte escape. */
const SIMPLE = new RegExp(`${ESC}[@-Z\\\\-_]`, 'g');

/** The 6x6x6 cube and the grey ramp of the 256-colour table, as true colours. */
function palette256(n: number): AnsiColor {
  if (n < 16) return { kind: 'named', index: n };
  if (n >= 232) {
    const level = 8 + (n - 232) * 10;
    return { kind: 'rgb', r: level, g: level, b: level };
  }
  const cube = n - 16;
  const step = (value: number) => (value === 0 ? 0 : 55 + value * 40);
  return { kind: 'rgb', r: step(Math.floor(cube / 36)), g: step(Math.floor(cube / 6) % 6), b: step(cube % 6) };
}

/** Applies one `SGR` parameter list to a style. */
function applyParams(style: AnsiStyle, params: readonly number[]): AnsiStyle {
  let next: AnsiStyle = { ...style };
  for (let at = 0; at < params.length; at += 1) {
    const code = params[at] ?? 0;
    if (code === 0) next = {};
    else if (code === 1) next.bold = true;
    else if (code === 2) next.dim = true;
    else if (code === 3) next.italic = true;
    else if (code === 4) next.underline = true;
    else if (code === 7) next.inverse = true;
    else if (code === 22) {
      delete next.bold;
      delete next.dim;
    } else if (code === 23) delete next.italic;
    else if (code === 24) delete next.underline;
    else if (code === 27) delete next.inverse;
    else if (code >= 30 && code <= 37) next.fg = { kind: 'named', index: code - 30 };
    else if (code === 39) delete next.fg;
    else if (code >= 40 && code <= 47) next.bg = { kind: 'named', index: code - 40 };
    else if (code === 49) delete next.bg;
    else if (code >= 90 && code <= 97) next.fg = { kind: 'named', index: code - 90 + 8 };
    else if (code >= 100 && code <= 107) next.bg = { kind: 'named', index: code - 100 + 8 };
    else if (code === 38 || code === 48) {
      const target = code === 38 ? 'fg' : 'bg';
      const mode = params[at + 1];
      if (mode === 5) {
        next[target] = palette256(params[at + 2] ?? 0);
        at += 2;
      } else if (mode === 2) {
        next[target] = { kind: 'rgb', r: params[at + 2] ?? 0, g: params[at + 3] ?? 0, b: params[at + 4] ?? 0 };
        at += 4;
      }
    }
  }
  return next;
}

const sameStyle = (one: AnsiStyle, other: AnsiStyle): boolean => JSON.stringify(one) === JSON.stringify(other);

/** What stays of a line after carriage returns: the text after the last one that is followed by anything. */
function settleReturns(text: string): string {
  if (!text.includes('\r')) return text;
  return text
    .split('\n')
    .map((line) => {
      const parts = line.split('\r').filter((part, index, all) => part !== '' || index === all.length - 1);
      // `\r\n` leaves an empty last part and keeps the line; a bare `\r` rewrites it from the start.
      const last = parts.at(-1) ?? '';
      return last === '' && parts.length > 1 ? (parts.at(-2) ?? '') : last;
    })
    .join('\n');
}

/**
 * Reads text with ANSI escapes into styled runs. A run that continues in the same style is one span,
 * and text without escapes comes back as one span with no style at all.
 */
export function parseAnsi(input: string): AnsiSpan[] {
  const text = settleReturns(input.replace(OSC, '').replace(SIMPLE, (match) => (match === `${ESC}[` ? match : '')));
  const spans: AnsiSpan[] = [];
  let style: AnsiStyle = {};
  let at = 0;
  const push = (chunk: string) => {
    if (chunk === '') return;
    const last = spans.at(-1);
    if (last !== undefined && sameStyle(last.style, style)) last.text += chunk;
    else spans.push({ text: chunk, style: { ...style } });
  };
  CSI.lastIndex = 0;
  for (let found = CSI.exec(text); found !== null; found = CSI.exec(text)) {
    push(text.slice(at, found.index));
    at = found.index + found[0].length;
    // Only `m` sets how text looks; a private mode (`?`) or any other final byte is dropped.
    if (found[3] === 'm' && !(found[1] ?? '').startsWith('?')) {
      const params = (found[1] ?? '')
        .split(/[;:]/)
        .map((part) => (part === '' ? 0 : Number(part)))
        .filter((value) => Number.isFinite(value));
      style = applyParams(style, params.length === 0 ? [0] : params);
    }
  }
  push(text.slice(at));
  return spans;
}

/** The text alone, as a person would copy it. */
export function stripAnsi(input: string): string {
  return parseAnsi(input)
    .map((span) => span.text)
    .join('');
}
