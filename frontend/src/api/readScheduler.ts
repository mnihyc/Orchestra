/** Shared source cuts, single flights and native aggregate ownership. No request counters. */
export class ReadScheduler {
  private cut = 0;
  private flights = new Map<string, { cut: number; promise: Promise<unknown> }>();
  private supplied = new Map<string, { cut: number; owner: readonly unknown[] }>();
  private key(key: readonly unknown[]): string { return JSON.stringify(key); }

  /** A reader/source change retires prior suppliers, including transports that ignore abort. */
  advance(): void { this.cut += 1; this.flights.clear(); }
  version(): number { return this.cut; }
  current(cut: number): boolean { return cut === this.cut; }

  run<T>(key: readonly unknown[], read: (current: () => boolean) => Promise<T>): Promise<T> {
    const id = this.key(key);
    const previous = this.flights.get(id);
    if (previous?.cut === this.cut) return previous.promise as Promise<T>;
    const cut = this.cut;
    const promise = Promise.resolve().then(() => read(() => this.current(cut)));
    const flight = { cut, promise };
    this.flights.set(id, flight);
    const done = () => { if (this.flights.get(id) === flight) this.flights.delete(id); };
    void promise.then(done, done);
    return promise;
  }

  /** The aggregate supplied the actual native query, with its existing key and cursor. */
  supply(owner: readonly unknown[], keys: readonly (readonly unknown[])[]): void {
    for (const key of keys) this.supplied.set(this.key(key), { cut: this.cut, owner });
  }
  owns(key: readonly unknown[]): boolean { return this.supplied.get(this.key(key))?.cut === this.cut; }
  owner(key: readonly unknown[]): readonly unknown[] | undefined { return this.supplied.get(this.key(key))?.owner; }
  release(owner: readonly unknown[]): void {
    const id = this.key(owner);
    for (const [key, supplied] of this.supplied) if (this.key(supplied.owner) === id) this.supplied.delete(key);
  }
}

export const scheduledReads = new ReadScheduler();

/** Automatic observers consume the aggregate; explicit refetch/next-page still calls the native reader. */
export const suppliedRead = (key: readonly unknown[]) => scheduledReads.owns(key)
  ? { refetchOnMount: false as const, retryOnMount: false, staleTime: Infinity }
  : {};
