/** What a time-series chart does with the lines it is given before it draws them: lines laid on one list of moments, a clean top for the scale, a few rows for the table that says the same, and the figures under the legend.
 *
 * Pure functions, so that what a chart shows can be tested without drawing one.
 */

/** One line of a chart: a series by one name, its points as a moment in milliseconds and a value. */
export interface ChartLine {
  /** Stable for the thing the line is of, so that the line keeps its colour when others come and go. */
  id: string;
  label: string;
  /** Which of the chart's colours the line takes, from 1; the caller gives them by what a line is, never by its rank. */
  slot: number;
  points: readonly (readonly [number, number])[];
}

/** The lines on one list of moments: every line has a value at each moment or `null` where it has none. */
export interface Aligned {
  /** Moments, in milliseconds, increasing. */
  xs: number[];
  /** For each line, its value at each moment of `xs`. */
  ys: (number | null)[][];
}

export function alignLines(lines: readonly ChartLine[]): Aligned {
  const stamps = new Set<number>();
  for (const line of lines) for (const [at] of line.points) stamps.add(at);
  const xs = [...stamps].sort((a, b) => a - b);
  const index = new Map(xs.map((at, position) => [at, position] as const));
  const ys = lines.map((line) => {
    const column: (number | null)[] = Array.from({ length: xs.length }, () => null);
    for (const [at, value] of line.points) {
      const place = index.get(at);
      if (place !== undefined) column[place] = value;
    }
    return column;
  });
  return { xs, ys };
}

/** The top of the scale: a little above the largest value, so that the line does not touch the frame; and one for a chart that is all zero. */
export function scaleTop(largest: number | null | undefined): number {
  if (largest === null || largest === undefined || !Number.isFinite(largest) || largest <= 0) return 1;
  return largest * 1.08;
}

/** The largest value of any line, or `null` when there is none. */
export function largestOf(aligned: Aligned): number | null {
  let largest: number | null = null;
  for (const column of aligned.ys) {
    for (const value of column) {
      if (value !== null && (largest === null || value > largest)) largest = value;
    }
  }
  return largest;
}

/** What is said of one line under the legend: how much it is now, and the most it has been. */
export interface LineFigures {
  now: number | null;
  peak: number | null;
}

export function figuresOf(line: readonly (number | null)[]): LineFigures {
  let now: number | null = null;
  let peak: number | null = null;
  for (const value of line) {
    if (value === null) continue;
    now = value;
    if (peak === null || value > peak) peak = value;
  }
  return { now, peak };
}

/** The most rows the table view of a chart has: a screen reader and an eye read a page of figures, not a thousand. */
export const TABLE_ROWS = 24;

/** The indexes of the moments the table view shows: evenly spread, the first and the last among them. */
export function sampleRows(count: number, most: number = TABLE_ROWS): number[] {
  if (count <= most) return Array.from({ length: count }, (_, at) => at);
  const rows = new Set<number>();
  for (let step = 0; step < most; step += 1) rows.add(Math.round((step * (count - 1)) / (most - 1)));
  return [...rows];
}

/** The colour `hex` at a share of its strength, as a CSS colour: the wash under a line. */
export function withAlpha(hex: string, alpha: number): string {
  const match = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})$/i.exec(hex.trim());
  if (match === null) return hex;
  const [, red, green, blue] = match;
  return `rgba(${parseInt(red ?? '0', 16)}, ${parseInt(green ?? '0', 16)}, ${parseInt(blue ?? '0', 16)}, ${alpha})`;
}

/** The index of the moment nearest to `at`, or `null` when there are none. */
export function nearestIndex(xs: readonly number[], at: number): number | null {
  if (xs.length === 0) return null;
  let low = 0;
  let high = xs.length - 1;
  while (low < high) {
    const middle = (low + high) >> 1;
    if ((xs[middle] ?? 0) < at) low = middle + 1;
    else high = middle;
  }
  const after = xs[low] ?? at;
  const before = xs[low - 1];
  if (before !== undefined && at - before <= after - at) return low - 1;
  return low;
}
