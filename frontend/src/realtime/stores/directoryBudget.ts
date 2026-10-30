/** One retained budget for directory windows and their selector/expansion metadata. */
/** Four native maximum pages (5000 rows / 2MiB each); encoded size is not a heap-size claim. */
export const DIRECTORY_ROWS_MAX = 20_000;
export const DIRECTORY_BYTES_MAX = 8 * 1024 * 1024;
export const DIRECTORY_RECORDS_MAX = 256;
export const DIRECTORY_METADATA_MAX = 2 * 1024 * 1024;
/** Matches the existing unused-query retention window; expiry is checked only on actual use. */
export const DIRECTORY_IDLE_MS = 5 * 60_000;

export interface DirectoryCost { rows: number; bytes: number; metadata: number }
interface Held<Value> { value: Value; cost: DirectoryCost; pins: number; used: number }
export interface DirectoryTotals extends DirectoryCost { records: number }
export type DirectoryBudgetRefusal = 'rows' | 'bytes' | 'records' | 'metadata';
const ZERO: DirectoryTotals = { rows: 0, bytes: 0, metadata: 0, records: 0 };

/** A pin owns a complete window. Pressure may retire only unowned records, never pieces of a page. */
export class DirectoryBudget<Value> {
  private readonly held = new Map<string, Held<Value>>();
  private totals: DirectoryTotals = { ...ZERO };
  private refused: DirectoryBudgetRefusal | null = null;
  private readonly retired: (key: string, value: Value) => void;
  private readonly now: () => number;
  constructor(retired: (key: string, value: Value) => void, now: () => number = Date.now) { this.retired = retired; this.now = now; }

  usage(): Readonly<DirectoryTotals> { return { ...this.totals }; }
  failure(): DirectoryBudgetRefusal | null { return this.refused; }
  keys(): readonly string[] { return [...this.held.keys()]; }
  get(key: string): Value | undefined {
    this.expire();
    const found = this.held.get(key);
    if (found === undefined) return undefined;
    found.used = this.now(); return found.value;
  }
  peek(key: string): Value | undefined { return this.held.get(key)?.value; }

  pin(key: string): (() => void) | null {
    this.expire();
    const found = this.held.get(key);
    if (found === undefined) return null;
    found.pins += 1; found.used = this.now();
    let released = false;
    return () => {
      if (released) return;
      released = true; found.pins -= 1; found.used = this.now();
    };
  }

  /** Pressure evicts nothing unless the whole replacement fits. Idle expiry remains independent. */
  put(key: string, value: Value, cost: DirectoryCost): boolean {
    if (!Object.values(cost).every((one) => Number.isSafeInteger(one) && one >= 0)) throw new RangeError('Invalid directory accounting');
    this.refused = this.excess({ ...cost, records: 1 });
    if (this.refused !== null) return false;
    this.expire();
    const previous = this.held.get(key);
    const planned = { rows: this.totals.rows + cost.rows - (previous?.cost.rows ?? 0),
      bytes: this.totals.bytes + cost.bytes - (previous?.cost.bytes ?? 0),
      metadata: this.totals.metadata + cost.metadata - (previous?.cost.metadata ?? 0),
      records: this.totals.records + (previous === undefined ? 1 : 0) };
    const victims: string[] = [];
    const candidates = [...this.held].filter(([id, row]) => id !== key && row.pins === 0).sort((one, other) => one[1].used - other[1].used);
    for (const [id, row] of candidates) {
      if (this.fits(planned)) break;
      victims.push(id); planned.rows -= row.cost.rows; planned.bytes -= row.cost.bytes;
      planned.metadata -= row.cost.metadata; planned.records -= 1;
    }
    this.refused = this.excess(planned);
    if (this.refused !== null) return false;
    for (const id of victims) this.remove(id);
    if (previous !== undefined) {
      this.subtract(previous.cost); previous.value = value; previous.cost = cost; previous.used = this.now();
    } else {
      this.held.set(key, { value, cost, pins: 0, used: this.now() }); this.totals.records += 1;
    }
    this.add(cost); return true;
  }

  /** Explicit authority/binding retirement overrides a pin; its later release stays harmless. */
  remove(key: string): void {
    const found = this.held.get(key);
    if (found === undefined) return;
    this.held.delete(key); this.totals.records -= 1; this.subtract(found.cost);
    this.retired(key, found.value);
  }
  clear(): void { for (const key of this.held.keys()) this.remove(key); }
  expire(): void {
    const before = this.now() - DIRECTORY_IDLE_MS;
    for (const [key, row] of this.held) if (row.pins === 0 && row.used <= before) this.remove(key);
  }
  private fits(total: DirectoryTotals): boolean {
    return this.excess(total) === null;
  }
  private excess(total: DirectoryTotals): DirectoryBudgetRefusal | null {
    if (total.rows > DIRECTORY_ROWS_MAX) return 'rows';
    if (total.bytes > DIRECTORY_BYTES_MAX) return 'bytes';
    if (total.records > DIRECTORY_RECORDS_MAX) return 'records';
    return total.metadata > DIRECTORY_METADATA_MAX ? 'metadata' : null;
  }
  private add(cost: DirectoryCost): void { this.totals.rows += cost.rows; this.totals.bytes += cost.bytes; this.totals.metadata += cost.metadata; }
  private subtract(cost: DirectoryCost): void { this.totals.rows -= cost.rows; this.totals.bytes -= cost.bytes; this.totals.metadata -= cost.metadata; }
}
