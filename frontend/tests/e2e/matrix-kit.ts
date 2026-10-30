/** The combination matrix's generator for the browser tests, the same as the Rust one (`crates/control/engine/tests/matrix_kit`):
 * every pair of values that can meet in at least one case (a greedy cover over seeded candidates), the whole product of
 * the dimensions whose values interact, and seeded random combinations; each case named `<matrix>/<kind>/<seed>/<index>`.
 *
 * Replay one case with `ORCH_MATRIX_CASE=<matrix>/<kind>/<seed>/<index>`; `ORCH_MATRIX_SEED=<number>` replaces the seed of
 * the random cases and `ORCH_MATRIX_RANDOM=<n>` their number. Numbers are drawn by SplitMix64 on 64-bit integers, so the
 * cases are the same on every machine. */

export interface Dim {
  name: string;
  values: readonly string[];
}

export interface Case {
  id: string;
  kind: 'pairs' | 'full' | 'random';
  values: Record<string, string>;
}

const MASK = (1n << 64n) - 1n;

/** SplitMix64: a deterministic stream of numbers from a seed. */
export class Seeded {
  private state: bigint;

  constructor(seed: number | bigint) {
    this.state = BigInt(seed) & MASK;
  }

  next(): bigint {
    this.state = (this.state + 0x9e3779b97f4a7c15n) & MASK;
    let z = this.state;
    z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & MASK;
    z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & MASK;
    return z ^ (z >> 31n);
  }

  below(bound: number): number {
    return Number(this.next() % BigInt(Math.max(1, bound)));
  }

  pick<T>(items: readonly T[]): T {
    const item = items[this.below(items.length)];
    if (item === undefined) throw new Error('nothing to pick from');
    return item;
  }
}

type Row = number[];

function product(dims: readonly Dim[]): Row[] {
  let rows: Row[] = [[]];
  for (const dim of dims) {
    rows = rows.flatMap((row) => dim.values.map((_, at) => [...row, at]));
  }
  return rows;
}

function pairsOf(row: Row): string[] {
  const pairs: string[] = [];
  for (let i = 0; i < row.length; i += 1) {
    for (let j = i + 1; j < row.length; j += 1) pairs.push(`${i}:${row[i]}|${j}:${row[j]}`);
  }
  return pairs;
}

function named(matrix: string, kind: Case['kind'], seed: number, index: number, dims: readonly Dim[], row: Row): Case {
  const values: Record<string, string> = {};
  dims.forEach((dim, at) => {
    const value = dim.values[row[at] ?? 0];
    if (value === undefined) throw new Error(`no value ${String(row[at])} of ${dim.name}`);
    values[dim.name] = value;
  });
  return { id: `${matrix}/${kind}/${seed}/${index}`, kind, values };
}

/** The cases of a matrix: the pair cover, the full products of each set of interacting dimensions, and random cases. */
export function cases(
  matrix: string,
  dims: readonly Dim[],
  rule: (values: Record<string, string>) => boolean,
  options: { pairSeed: number; interacting: readonly (readonly string[])[]; randomSeed: number; random: number },
): Case[] {
  const valid = product(dims).filter((row) => rule(named(matrix, 'pairs', 0, 0, dims, row).values));
  if (valid.length === 0) throw new Error(`the matrix ${matrix} has no valid combination`);
  // The pair cover.
  const missing = new Set(valid.flatMap(pairsOf));
  const seeded = new Seeded(options.pairSeed);
  const chosen: Row[] = [];
  for (;;) {
    const target = [...missing].sort()[0];
    if (target === undefined) break;
    const holding = valid.filter((row) => pairsOf(row).includes(target));
    const candidates = [...Array.from({ length: 24 }, () => seeded.pick(holding)), ...Array.from({ length: 24 }, () => seeded.pick(valid))];
    const first = candidates[0];
    if (first === undefined) break;
    let best: Row = first;
    let gain = -1;
    for (const candidate of candidates) {
      const adds = pairsOf(candidate).filter((pair) => missing.has(pair)).length;
      if (adds > gain) {
        best = candidate;
        gain = adds;
      }
    }
    for (const pair of pairsOf(best)) missing.delete(pair);
    chosen.push(best);
  }
  const all: Case[] = chosen.map((row, index) => named(matrix, 'pairs', options.pairSeed, index, dims, row));
  // The full products.
  options.interacting.forEach((names, n) => {
    const at = names.map((name) => dims.findIndex((dim) => dim.name === name));
    const seed = options.pairSeed + 1 + n;
    const pick = new Seeded(seed);
    const groups = new Map<string, Row[]>();
    for (const row of valid) {
      const key = at.map((i) => row[i]).join(',');
      groups.set(key, [...(groups.get(key) ?? []), row]);
    }
    [...groups.keys()].sort().forEach((key, index) => {
      all.push(named(matrix, 'full', seed, index, dims, pick.pick(groups.get(key) ?? [])));
    });
  });
  // The random cases.
  const randomSeed = process.env.ORCH_MATRIX_SEED === undefined ? options.randomSeed : Number(process.env.ORCH_MATRIX_SEED);
  const count = process.env.ORCH_MATRIX_RANDOM === undefined ? options.random : Number(process.env.ORCH_MATRIX_RANDOM);
  const draw = new Seeded(randomSeed);
  for (let index = 0; index < count; index += 1) all.push(named(matrix, 'random', randomSeed, index, dims, draw.pick(valid)));
  const wanted = process.env.ORCH_MATRIX_CASE;
  return wanted === undefined || wanted === '' ? all : all.filter((each) => each.id === wanted);
}

/** A case's title for the list of tests: its name and its values. */
export function title(each: Case): string {
  return `${each.id} ${Object.entries(each.values)
    .map(([name, value]) => `${name}=${value}`)
    .join(' ')}`;
}
