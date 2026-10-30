/** A QR code for a short text: the dark and light squares of the smallest symbol that holds it.
 *
 * Only what a link to scan needs is here: the text is stored as bytes, with the error correction of
 * the middle level (a symbol that is dirty or crooked still reads), in the sizes up to version 10
 * (57 squares across, 213 bytes). A text that does not fit gets no symbol, and the caller shows the
 * text itself. Nothing is fetched and nothing is drawn: `qrModules` answers a grid of booleans.
 */

/** The largest version drawn: past it a text is no link to be scanned from a screen. */
const MAX_VERSION = 10;

/**
 * Data codewords and the blocks they are split into, by version, at the middle level of error
 * correction: [error codewords of a block, blocks of the first kind, data codewords of such a block,
 * blocks of the second kind, data codewords of such a block].
 */
const BLOCKS: readonly (readonly [number, number, number, number, number])[] = [
  [10, 1, 16, 0, 0],
  [16, 1, 28, 0, 0],
  [26, 1, 44, 0, 0],
  [18, 2, 32, 0, 0],
  [24, 2, 43, 0, 0],
  [16, 4, 27, 0, 0],
  [18, 4, 31, 0, 0],
  [22, 2, 38, 2, 39],
  [22, 3, 36, 2, 37],
  [26, 4, 43, 1, 44],
];

/** Where the alignment patterns' centres lie along one side, by version (version 1 has none). */
const ALIGNMENT: readonly (readonly number[])[] = [[], [6, 18], [6, 22], [6, 26], [6, 30], [6, 34], [6, 22, 38], [6, 24, 42], [6, 26, 46], [6, 28, 50]];

/** The squares left over after the codewords, which are filled with light ones: by version. */
const REMAINDER_BITS: readonly number[] = [0, 7, 7, 7, 7, 7, 0, 0, 0, 0];

/** The primitive polynomial of the field the error correction is counted in: x^8 + x^4 + x^3 + x^2 + 1. */
const FIELD_POLYNOMIAL = 0x11d;

/** The format information's generator (x^10 + x^8 + x^5 + x^4 + x^2 + x + 1) and the pattern it is hidden under. */
const FORMAT_GENERATOR = 0x537;
const FORMAT_MASK = 0x5412;

/** The version information's generator (x^12 + x^11 + x^10 + x^9 + x^8 + x^5 + x^2 + 1). */
const VERSION_GENERATOR = 0x1f25;

/** The two bits that name the middle level of error correction in the format information. */
const LEVEL_BITS = 0b00;

/** Penalty weights of the four rules by which the best of the eight masks is chosen. */
const PENALTY_RUN = 3;
const PENALTY_BLOCK = 3;
const PENALTY_FINDER = 40;
const PENALTY_BALANCE = 10;

const EXPONENT = new Uint8Array(512);
const LOGARITHM = new Uint8Array(256);
{
  let value = 1;
  for (let power = 0; power < 255; power += 1) {
    EXPONENT[power] = value;
    LOGARITHM[value] = power;
    value <<= 1;
    if (value & 0x100) value ^= FIELD_POLYNOMIAL;
  }
  for (let power = 255; power < 512; power += 1) EXPONENT[power] = EXPONENT[power - 255] ?? 0;
}

function multiply(a: number, b: number): number {
  if (a === 0 || b === 0) return 0;
  return EXPONENT[(LOGARITHM[a] ?? 0) + (LOGARITHM[b] ?? 0)] ?? 0;
}

/** The error correction codewords of one block of data. */
function remainder(data: readonly number[], count: number): number[] {
  // The generator polynomial: the product of (x - 2^i) for i below the count.
  let generator: number[] = [1];
  for (let power = 0; power < count; power += 1) {
    const next: number[] = new Array<number>(generator.length + 1).fill(0);
    for (let at = 0; at < generator.length; at += 1) {
      next[at] = (next[at] ?? 0) ^ (generator[at] ?? 0);
      next[at + 1] = (next[at + 1] ?? 0) ^ multiply(generator[at] ?? 0, EXPONENT[power] ?? 0);
    }
    generator = next;
  }
  const rest: number[] = new Array<number>(count).fill(0);
  for (const byte of data) {
    const lead = byte ^ (rest.shift() ?? 0);
    rest.push(0);
    for (let at = 0; at < count; at += 1) rest[at] = (rest[at] ?? 0) ^ multiply(generator[at + 1] ?? 0, lead);
  }
  return rest;
}

/** The bits of `value`, `length` of them, most significant first, added to `bits`. */
function push(bits: number[], value: number, length: number): void {
  for (let at = length - 1; at >= 0; at -= 1) bits.push((value >>> at) & 1);
}

/** The data codewords that hold the bytes of a text in byte mode, padded to `capacity` codewords. */
function dataCodewords(bytes: Uint8Array, version: number, capacity: number): number[] {
  const bits: number[] = [];
  push(bits, 0b0100, 4);
  push(bits, bytes.length, version < 10 ? 8 : 16);
  for (const byte of bytes) push(bits, byte, 8);
  // A terminator of up to four zeros, then zeros up to a whole codeword.
  push(bits, 0, Math.min(4, capacity * 8 - bits.length));
  while (bits.length % 8 !== 0) bits.push(0);
  const words: number[] = [];
  for (let at = 0; at < bits.length; at += 8) {
    let word = 0;
    for (let bit = 0; bit < 8; bit += 1) word = (word << 1) | (bits[at + bit] ?? 0);
    words.push(word);
  }
  // The padding codewords the standard names, in turn.
  for (let pad = 0; words.length < capacity; pad += 1) words.push(pad % 2 === 0 ? 0xec : 0x11);
  return words;
}

/** The codewords of a symbol: the data cut into blocks, each with its error correction, interleaved. */
function interleave(words: readonly number[], version: number): number[] {
  const [errorCount, firstBlocks, firstSize, secondBlocks, secondSize] = BLOCKS[version - 1] ?? BLOCKS[0] ?? [0, 0, 0, 0, 0];
  const sizes = [...new Array<number>(firstBlocks).fill(firstSize), ...new Array<number>(secondBlocks).fill(secondSize)];
  const data: number[][] = [];
  const errors: number[][] = [];
  let from = 0;
  for (const size of sizes) {
    const block = words.slice(from, from + size);
    from += size;
    data.push(block);
    errors.push(remainder(block, errorCount));
  }
  const out: number[] = [];
  for (let at = 0; at < Math.max(firstSize, secondSize); at += 1) {
    for (const block of data) if (at < block.length) out.push(block[at] ?? 0);
  }
  for (let at = 0; at < errorCount; at += 1) for (const block of errors) out.push(block[at] ?? 0);
  return out;
}

/** A grid of squares, with the ones that hold a fixed pattern marked so that data is not written over them. */
class Grid {
  readonly size: number;
  readonly dark: boolean[][];
  readonly fixed: boolean[][];

  constructor(version: number) {
    this.size = 17 + 4 * version;
    this.dark = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
    this.fixed = Array.from({ length: this.size }, () => new Array<boolean>(this.size).fill(false));
  }

  set(row: number, column: number, dark: boolean): void {
    const line = this.dark[row];
    const marked = this.fixed[row];
    if (line === undefined || marked === undefined || column < 0 || column >= this.size) return;
    line[column] = dark;
    marked[column] = true;
  }

  /** A finder pattern with its light border, its centre at (row, column). */
  finder(row: number, column: number): void {
    for (let dy = -4; dy <= 4; dy += 1) {
      for (let dx = -4; dx <= 4; dx += 1) {
        const distance = Math.max(Math.abs(dx), Math.abs(dy));
        this.set(row + dy, column + dx, distance !== 4 && distance !== 2);
      }
    }
  }

  alignment(row: number, column: number): void {
    for (let dy = -2; dy <= 2; dy += 1) {
      for (let dx = -2; dx <= 2; dx += 1) this.set(row + dy, column + dx, Math.max(Math.abs(dx), Math.abs(dy)) !== 1);
    }
  }
}

/** The bits of the format information for a mask: five data bits, ten of error correction, hidden. */
function formatBits(mask: number): number {
  const data = (LEVEL_BITS << 3) | mask;
  let code = data << 10;
  for (let at = 14; at >= 10; at -= 1) if ((code >>> at) & 1) code ^= FORMAT_GENERATOR << (at - 10);
  return ((data << 10) | code) ^ FORMAT_MASK;
}

/** The bits of the version information: six data bits and twelve of error correction. */
function versionBits(version: number): number {
  let code = version << 12;
  for (let at = 17; at >= 12; at -= 1) if ((code >>> at) & 1) code ^= VERSION_GENERATOR << (at - 12);
  return (version << 12) | code;
}

/** The fixed squares of a symbol: finders, timing, alignment, the dark square, and room for the format and version bits. */
function frame(version: number): Grid {
  const grid = new Grid(version);
  const last = grid.size - 1;
  grid.finder(3, 3);
  grid.finder(3, last - 3);
  grid.finder(last - 3, 3);
  for (let at = 8; at < last - 7; at += 1) {
    grid.set(6, at, at % 2 === 0);
    grid.set(at, 6, at % 2 === 0);
  }
  const centres = ALIGNMENT[version - 1] ?? [];
  for (const row of centres) {
    for (const column of centres) {
      // The three that would lie on a finder are left out.
      const onFinder = (row === 6 && column === 6) || (row === 6 && column === (centres.at(-1) ?? 0)) || (column === 6 && row === (centres.at(-1) ?? 0));
      if (!onFinder) grid.alignment(row, column);
    }
  }
  grid.set(last - 7, 8, true);
  // Room for the format bits, drawn light until the mask is chosen.
  for (let at = 0; at < 9; at += 1) {
    if (at !== 6) {
      grid.set(8, at, false);
      grid.set(at, 8, false);
    }
  }
  for (let at = 0; at < 8; at += 1) {
    grid.set(8, last - at, false);
    grid.set(last - at, 8, false);
  }
  grid.set(last - 7, 8, true);
  if (version >= 7) {
    const bits = versionBits(version);
    for (let at = 0; at < 18; at += 1) {
      const dark = ((bits >>> at) & 1) === 1;
      const a = Math.floor(at / 3);
      const b = (at % 3) + last - 10;
      grid.set(a, b, dark);
      grid.set(b, a, dark);
    }
  }
  return grid;
}

/** Whether the mask of that number flips the square at (row, column). */
function flips(mask: number, row: number, column: number): boolean {
  switch (mask) {
    case 0:
      return (row + column) % 2 === 0;
    case 1:
      return row % 2 === 0;
    case 2:
      return column % 3 === 0;
    case 3:
      return (row + column) % 3 === 0;
    case 4:
      return (Math.floor(row / 2) + Math.floor(column / 3)) % 2 === 0;
    case 5:
      return ((row * column) % 2) + ((row * column) % 3) === 0;
    case 6:
      return (((row * column) % 2) + ((row * column) % 3)) % 2 === 0;
    default:
      return (((row + column) % 2) + ((row * column) % 3)) % 2 === 0;
  }
}

/** The squares of a symbol with the codewords written in and one mask applied, format bits included. */
function drawn(version: number, codewords: readonly number[], mask: number): boolean[][] {
  const grid = frame(version);
  const size = grid.size;
  const bits: number[] = [];
  for (const word of codewords) push(bits, word, 8);
  push(bits, 0, REMAINDER_BITS[version - 1] ?? 0);

  // The data goes in two-square-wide strips from the lower right, up and down by turns, past the vertical timing line.
  let next = 0;
  let upward = true;
  for (let column = size - 1; column > 0; column -= 2) {
    if (column === 6) column -= 1;
    for (let step = 0; step < size; step += 1) {
      const row = upward ? size - 1 - step : step;
      for (let side = 0; side < 2; side += 1) {
        const at = column - side;
        if (grid.fixed[row]?.[at] === true) continue;
        const bit = (bits[next] ?? 0) === 1;
        next += 1;
        const line = grid.dark[row];
        if (line !== undefined) line[at] = bit !== flips(mask, row, at);
      }
    }
    upward = !upward;
  }

  // The format information is written twice: round the top left finder, and split between the other two.
  const format = formatBits(mask);
  const place = (row: number, column: number, bit: number) => {
    const line = grid.dark[row];
    if (line !== undefined) line[column] = ((format >>> bit) & 1) === 1;
  };
  for (let bit = 0; bit <= 5; bit += 1) place(bit, 8, bit);
  place(7, 8, 6);
  place(8, 8, 7);
  place(8, 7, 8);
  for (let bit = 9; bit < 15; bit += 1) place(8, 14 - bit, bit);
  for (let bit = 0; bit < 8; bit += 1) place(8, size - 1 - bit, bit);
  for (let bit = 8; bit < 15; bit += 1) place(size - 15 + bit, 8, bit);
  return grid.dark;
}

/** How badly a pattern of squares would read: long runs, blocks, finder look-alikes, and unbalanced colour. */
function penalty(modules: readonly (readonly boolean[])[]): number {
  const size = modules.length;
  let total = 0;
  const lines: boolean[][] = [];
  for (let at = 0; at < size; at += 1) {
    lines.push([...(modules[at] ?? [])]);
    lines.push(modules.map((row) => row[at] ?? false));
  }
  for (const line of lines) {
    let run = 1;
    for (let at = 1; at <= size; at += 1) {
      if (at < size && line[at] === line[at - 1]) run += 1;
      else {
        if (run >= 5) total += PENALTY_RUN + (run - 5);
        run = 1;
      }
    }
    // A finder's look-alike: 1 0 1 1 1 0 1 with four light squares on either side.
    const text = line.map((dark) => (dark ? '1' : '0')).join('');
    for (const pattern of ['10111010000', '00001011101']) {
      for (let from = text.indexOf(pattern); from !== -1; from = text.indexOf(pattern, from + 1)) total += PENALTY_FINDER;
    }
  }
  for (let row = 0; row < size - 1; row += 1) {
    for (let column = 0; column < size - 1; column += 1) {
      const colour = modules[row]?.[column];
      if (colour === modules[row]?.[column + 1] && colour === modules[row + 1]?.[column] && colour === modules[row + 1]?.[column + 1]) {
        total += PENALTY_BLOCK;
      }
    }
  }
  const dark = modules.reduce((sum, row) => sum + row.filter(Boolean).length, 0);
  const share = (dark * 100) / (size * size);
  total += Math.floor(Math.abs(share - 50) / 5) * PENALTY_BALANCE;
  return total;
}

/**
 * The squares of the symbol that holds `text`, row by row, `true` for a dark one; `null` when the
 * text is longer than the largest symbol here holds.
 */
export function qrModules(text: string): boolean[][] | null {
  const bytes = new TextEncoder().encode(text);
  for (let version = 1; version <= MAX_VERSION; version += 1) {
    const [, firstBlocks, firstSize, secondBlocks, secondSize] = BLOCKS[version - 1] ?? [0, 0, 0, 0, 0];
    const capacity = firstBlocks * firstSize + secondBlocks * secondSize;
    const countBits = version < 10 ? 8 : 16;
    if (4 + countBits + bytes.length * 8 > capacity * 8) continue;
    const codewords = interleave(dataCodewords(bytes, version, capacity), version);
    let best: boolean[][] | null = null;
    let worst = Number.POSITIVE_INFINITY;
    for (let mask = 0; mask < 8; mask += 1) {
      const modules = drawn(version, codewords, mask);
      const cost = penalty(modules);
      if (cost < worst) {
        worst = cost;
        best = modules;
      }
    }
    return best;
  }
  return null;
}
