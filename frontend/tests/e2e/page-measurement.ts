/** Accounting is based on request starts, independently of when their answers finish. */
export interface MeasuredRequest { path: string; kind: string; start: number; end: number | null; wire: number | null; decoded: number | null; failed: boolean }
export class RequestLedger<Key> {
  private entries = new Map<Key, MeasuredRequest>();
  begin(key: Key, path: string, start: number, kind: string): void {
    this.entries.set(key, { path, kind, start, end: null, wire: null, decoded: null, failed: false });
  }
  finish(key: Key, result: Pick<MeasuredRequest, 'end' | 'wire' | 'decoded' | 'failed'>): void {
    const entry = this.entries.get(key);
    if (entry !== undefined) Object.assign(entry, result);
  }
  before(content: number, since = 0): MeasuredRequest[] { return [...this.entries.values()].filter((entry) => entry.start >= since && entry.start <= content); }
}

export type PageOutcome = 'waiting' | 'ready' | 'redirect' | 'error';
export function sameAddress(wanted: string, actual: string): boolean {
  const expected = new URL(wanted), current = new URL(actual);
  return expected.origin === current.origin && expected.pathname.replace(/\/$/, '') === current.pathname.replace(/\/$/, '')
    && [...expected.searchParams].every(([key, value]) => current.searchParams.get(key) === value);
}
export function pageOutcome(wanted: string, actual: string, marker: string | null, visible: boolean): PageOutcome {
  if (!sameAddress(wanted, actual)) return 'redirect';
  if (visible && marker === 'error') return 'error';
  return visible && marker === 'ready' ? 'ready' : 'waiting';
}
export interface RunNumbers { scenario: 'cold' | 'repeat' | 'switch'; calls: number; bytes: number; content: number }
export function compareRuns(current: RunNumbers, accepted: RunNumbers): Omit<RunNumbers, 'scenario'> {
  if (current.scenario !== accepted.scenario) throw new Error('The accepted scenario differs from the measured scenario');
  return { calls: current.calls - accepted.calls, bytes: current.bytes - accepted.bytes, content: current.content - accepted.content };
}
