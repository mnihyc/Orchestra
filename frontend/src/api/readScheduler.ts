/** Shared source cuts, single flights and native aggregate ownership. No request counters. */
export class ReadScheduler {
  private cut = 0;
  private flights = new Map<string, { cut: number; promise: Promise<unknown> }>();
  private supplied = new Map<string, { cut: number; owner: readonly unknown[]; ownerId: string; resource: string | null }>();
  private members = new Map<string, Set<string>>();
  private resources = new Map<string | null, number>();
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
    const ownerId = this.key(owner);
    for (const key of keys) {
      const id = this.key(key);
      this.remove(id);
      const resource = typeof key[0] === 'string' ? key[0] : null;
      const members = this.members.get(ownerId) ?? new Set<string>();
      members.add(id);
      this.members.set(ownerId, members);
      this.resources.set(resource, (this.resources.get(resource) ?? 0) + 1);
      this.supplied.set(id, { cut: this.cut, owner, ownerId, resource });
    }
  }
  private lookup(key: readonly unknown[]) {
    // Non-string leading values can serialize to strings too. Keep the exact lookup for them.
    if (typeof key[0] === 'string' && !this.resources.has(key[0]) && !this.resources.has(null)) return undefined;
    return this.supplied.get(this.key(key));
  }
  owns(key: readonly unknown[], ownerKind?: string): boolean {
    const held = this.lookup(key);
    return held?.cut === this.cut && (ownerKind === undefined || held.owner[0] === ownerKind);
  }
  owner(key: readonly unknown[]): readonly unknown[] | undefined { return this.lookup(key)?.owner; }
  private remove(id: string): void {
    const held = this.supplied.get(id);
    if (held === undefined) return;
    this.supplied.delete(id);
    const members = this.members.get(held.ownerId);
    members?.delete(id);
    if (members?.size === 0) this.members.delete(held.ownerId);
    const count = this.resources.get(held.resource) ?? 0;
    if (count <= 1) this.resources.delete(held.resource);
    else this.resources.set(held.resource, count - 1);
  }
  release(owner: readonly unknown[]): void {
    const members = this.members.get(this.key(owner));
    if (members !== undefined) for (const id of members) this.remove(id);
  }
}

export const scheduledReads = new ReadScheduler();

/** Automatic observers consume the aggregate; explicit refetch/next-page still calls the native reader. */
export const suppliedRead = (key: readonly unknown[]) => scheduledReads.owns(key)
  ? { refetchOnMount: false as const, retryOnMount: false, staleTime: Infinity }
  : {};

/** A tab's initial consumer takes over its already-started read without immediately starting it twice. */
export const startingRead = (key: readonly unknown[]) => scheduledReads.owns(key, 'page-read')
  ? { refetchOnMount: false as const, retryOnMount: false, staleTime: Infinity }
  : {};
