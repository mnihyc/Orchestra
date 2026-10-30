/** Bounded shared directory observations, with independent selected pages and owned native demand. */
import { ApiError } from '@/api/client';
import type { Door } from '@/api/workspace';
import type { DirData, DirEntry, DirectoryWatchError, DirListing, FileListSource } from '@/generated/api-types';
import { NetworkError } from '@/lib/requests';
import type { FeedFrame, Subscription } from '@/realtime/socket';
import { DirectoryBudget } from './directoryBudget';
import type { DirectoryBudgetRefusal } from './directoryBudget';
import { DirectoryReads } from './directoryReads';
import { EMPTY, directoryFeed, directoryKey, doorKey, sortEntries } from './directoryTypes';
import type { DirectoryAdmission, DirectoryCapacity, DirectoryCapacityReason, DirectoryLease, DirectoryNavigation, DirectoryOpen, DirectoryOpened, DirectoryState, DirectoryWindow } from './directoryTypes';

export const MOST_WATCHED = 24;
// Native public limits, not a smaller sliced browser window.
const NATIVE_ROWS = 5000, NATIVE_BYTES = 2 * 1024 * 1024;
// The existing realtime client-control frame allowance. Reserve it for status/failure metadata so
// a full expansion ledger cannot prevent recording a later refusal. It counts inside the 2MiB cap.
const CONTROL_BYTES = 16 * 1024;
const encoder = new TextEncoder();
const bytes = (value: unknown): number => encoder.encode(JSON.stringify(value)).length;
const CLOSED: DirectoryWindow = Object.freeze({ ...EMPTY, status: 'failed', watch: 'stale', restartRequired: true });
type Source = FileListSource;
interface Dependencies {
  reader: () => number | undefined;
  fetch: (door: Door, path: string, options: { source: Source; cursor?: string; limit: number; signal: AbortSignal }) => Promise<DirListing>;
  subscribe: (feed: string, options: { since: () => number | null; onFrame: (frame: FeedFrame) => void }) => Subscription;
  publish: (dirs: ReadonlyMap<string, DirectoryState>) => void;
  now?: () => number;
}
interface Selector { incoming: string | null; limit: number; source: Source; door: Door }
interface Base { key: string; id: number }
interface Directory extends Base {
  kind: 'directory'; door: Door; path: string; binding: string; reader: number | undefined; epoch: number;
  owners: number[]; actions: number; admitted: boolean; version: number; generation: number;
  state: DirectoryState; head: string | null; work: string | null;
  subscription: Subscription | null; install: number; seq: number | null; sealed: boolean; unpin: (() => void) | null;
}
interface View extends Base {
  kind: 'view'; directory: number; door: Door; pane: string; parent: number | null;
  selected: string | null; ordinal: number; trail: string[]; lost: boolean; expanded: readonly string[];
  stale: boolean; capacity: DirectoryCapacity | null; listener: (() => void) | null; snapshot: DirectoryWindow;
  unpin: (() => void) | null; pagePin: (() => void) | null;
}
type ImmutableListing = Omit<DirListing, 'entries'> & { readonly entries: readonly DirEntry[] };
interface Page extends Base { kind: 'page'; directory: number; selector: Selector; listing: ImmutableListing; ordinal: number }
interface Checkpoint extends Base { kind: 'checkpoint'; directory: number; page: string; selector: Selector; revision: string | null; ordinal: number }
interface Work extends Base {
  kind: 'work'; directory: number; selector: Selector; version: number; epoch: number; reader: number | undefined;
  path: string; binding: string;
  expected: { source: DirListing['source']; revision: string; asOf: string; from: string | null } | null;
  owners: { id: number; mode: 'first' | 'next' | 'previous' | 'restart'; ordinal: number; ancestors?: readonly number[] }[];
  joiners: number[];
  action: boolean; retired: boolean; controller: AbortController; promise: Promise<DirectoryNavigation>; unpin: (() => void) | null;
}
type Record = Directory | View | Page | Checkpoint | Work;
const controls = { status: 'loading', watch: 'connecting', navigation: 'loading', source: 'cached', capacity: 'directories',
  offline: false, invalidated: false, released: false, generation: Number.MAX_SAFE_INTEGER, ordinal: Number.MAX_SAFE_INTEGER };

/** All maps/queues here contain ledger-owned records or numeric references to them. Snapshots
 * reference the one charged page array. Control tags/numbers are charged at their finite maximum;
 * variable strings, errors, expansion and selector arrays are charged in full. Encoded != JS heap. */
export class DirectoryWindows {
  private readonly dependencies: Dependencies;
  private readonly now: () => number;
  private readonly budget: DirectoryBudget<Record>;
  private readonly reads = new DirectoryReads();
  private serial = 0;
  private epoch = 0;
  private batching = 0;
  private dirty = false;
  private pageFailure: DirectoryBudgetRefusal = 'metadata';
  constructor(dependencies: Dependencies) {
    this.dependencies = dependencies; this.now = dependencies.now ?? Date.now;
    this.budget = new DirectoryBudget((key, value) => this.retired(key, value), this.now);
  }
  usage() { return { ...this.budget.usage(), ...this.reads.usage(), admitted: this.directories().filter((row) => row.admitted).length }; }
  private id(): number { if (this.serial >= Number.MAX_SAFE_INTEGER) throw new RangeError('Directory identity exhausted'); return ++this.serial; }
  private capacity(reason: DirectoryCapacityReason): DirectoryCapacity { return { ok: false, reason, usage: this.budget.usage() }; }
  private records(): Record[] { return this.budget.keys().flatMap((key) => { const row = this.budget.peek(key); return row === undefined ? [] : [row]; }); }
  private directories(): Directory[] { return this.records().filter((row): row is Directory => row.kind === 'directory'); }
  private views(): View[] { return this.records().filter((row): row is View => row.kind === 'view'); }
  private directory(id: number): Directory | undefined { const value = this.budget.peek(`d:${id}`); return value?.kind === 'directory' ? value : undefined; }
  private view(id: number): View | undefined { const value = this.budget.peek(`v:${id}`); return value?.kind === 'view' ? value : undefined; }
  private page(key: string | null): Page | undefined { const value = key === null ? undefined : this.budget.peek(key); return value?.kind === 'page' ? value : undefined; }
  private work(key: string | null): Work | undefined { const value = key === null ? undefined : this.budget.peek(key); return value?.kind === 'work' ? value : undefined; }
  private metadata(row: Record): unknown {
    const base = [row.key, controls];
    switch (row.kind) {
      case 'page': { const { entries: _entries, ...metadata } = row.listing; return [...base, row.directory, row.selector, metadata]; }
      case 'checkpoint': return [...base, row.directory, row.page, row.selector, row.revision];
      case 'view': return [...base, row.directory, row.door, row.pane, row.parent, row.expanded];
      case 'work': return [...base, row.directory, row.path, row.binding, row.selector, row.expected, row.owners, row.joiners, true];
      case 'directory': return [...base, row.door, row.path, row.binding, row.reader,
        row.owners.map(() => Number.MAX_SAFE_INTEGER), Number.MAX_SAFE_INTEGER,
        `p:${Number.MAX_SAFE_INTEGER}`, `w:${Number.MAX_SAFE_INTEGER}`];
    }
  }
  private save(row: Record): boolean {
    const entries = row.kind === 'page' ? row.listing.entries : [];
    const control = row.kind === 'directory' ? this.control(row.state) : null;
    // A view's mutable reference trail can never exceed the shared 256 records. Reserve one
    // control-frame allowance for it, callbacks and the fixed-size capacity result. Expansion
    // strings remain separately charged in full and are admitted before replacing old intent.
    const cost = { rows: entries.length, bytes: entries.length === 0 ? 0 : bytes(entries),
      metadata: bytes(this.metadata(row)) + (row.kind === 'directory' ? Math.max(CONTROL_BYTES, bytes(control)) : row.kind === 'view' ? CONTROL_BYTES : 0) };
    if (this.budget.put(row.key, row, cost)) return true;
    if (row.kind !== 'directory' || bytes(control) <= CONTROL_BYTES) return false;
    // The full error may exceed the remaining metadata capacity. Preserve its normalized cause
    // and retryability, explicitly expose capacity, and never retain an uncharged raw problem.
    const error = row.state.error;
    const minimal = error instanceof ApiError ? new ApiError({ type: error.problem.type, code: error.code,
      title: '', detail: '', status: error.status, request_id: '' }, error.retryAt) : null;
    const failure = row.state.watchError;
    row.state = { ...row.state, error: minimal, ...(failure === null || failure === undefined ? {} : { watchError: { ...failure, detail: '', reason: null } }) };
    for (const view of this.views().filter((view) => view.directory === row.id)) view.capacity = this.capacity('metadata');
    return this.budget.put(row.key, row, { ...cost, metadata: bytes(this.metadata(row)) + CONTROL_BYTES });
  }
  private control(state: DirectoryState): unknown {
    const { entries: _entries, error, ...fields } = state;
    return { ...fields, error: error instanceof ApiError ? error.problem
      : error instanceof Error ? { name: error.name, message: error.message } : null };
  }
  private batch<T>(action: () => T): T {
    this.batching += 1;
    try { return action(); } finally { this.batching -= 1; if (this.batching === 0 && this.dirty) this.emit(); }
  }
  private changed(): void { this.dirty = true; if (this.batching === 0) this.emit(); }
  private emit(): void {
    this.dirty = false;
    const dirs = new Map<string, DirectoryState>();
    for (const dir of this.directories()) dirs.set(directoryKey(dir.door.environmentId, dir.path), this.headState(dir));
    for (const view of this.views()) view.snapshot = this.window(view);
    this.dependencies.publish(dirs);
    for (const view of this.views()) view.listener?.();
  }
  private headState(dir: Directory): DirectoryState {
    const page = this.page(dir.head);
    return page === undefined ? dir.state : { ...dir.state, entries: page.listing.entries, source: page.listing.source,
      asOf: page.listing.as_of, observedFrom: page.listing.observed_from ?? null, truncated: page.listing.truncated,
      revision: page.listing.revision ?? null, nextCursor: dir.state.invalidated === true ? null : page.listing.next_cursor ?? null,
      expiresAt: dir.state.invalidated === true ? null : page.listing.expires_at ?? null };
  }
  private expired(page: Page): boolean {
    const expiry = page.listing.expires_at;
    return expiry !== null && expiry !== undefined && Date.parse(expiry) <= this.now();
  }
  private window(view: View): DirectoryWindow {
    const dir = this.directory(view.directory);
    if (dir === undefined) return CLOSED;
    const page = this.page(view.selected), expired = page !== undefined && this.expired(page);
    const state = dir.state;
    const stale = view.stale || expired || dir.state.invalidated === true;
    const back = view.trail.at(-1), checkpoint = back === undefined ? undefined : this.budget.peek(back);
    const previous = checkpoint?.kind === 'checkpoint' ? this.page(checkpoint.page) : undefined;
    const canPrevious = checkpoint?.kind === 'checkpoint' && checkpoint.ordinal === view.ordinal - 1
      && (checkpoint.selector.incoming !== null || (previous !== undefined && !this.expired(previous)));
    return { ...state, ...(page === undefined ? {} : { entries: page.listing.entries, source: page.listing.source,
      asOf: page.listing.as_of, observedFrom: page.listing.observed_from ?? null, truncated: page.listing.truncated,
      revision: page.listing.revision ?? null, nextCursor: stale ? null : page.listing.next_cursor ?? null,
      expiresAt: stale ? null : page.listing.expires_at ?? null }), generation: dir.generation,
      page: page === undefined ? null : { incoming: page.selector.incoming, revision: page.listing.revision ?? null, ordinal: view.ordinal },
      navigation: this.work(dir.work)?.owners.some((owner) => owner.id === view.id) === true ? 'loading' : 'idle',
      canNext: !stale && (page?.listing.next_cursor ?? null) !== null && (page?.listing.revision ?? null) !== null,
      canPrevious: !stale && canPrevious, restartRequired: stale || view.lost || (view.ordinal > 1 && !canPrevious),
      capacity: view.capacity ?? (!dir.admitted ? this.capacity('directories') : null),
      expanded: view.expanded,
      invalidated: stale, watch: stale && dir.state.watch === 'live' ? 'stale' : dir.state.watch };
  }
  private ensure(door: Door, path: string, binding: string, waiting: boolean): Directory | DirectoryCapacity {
    this.budget.expire();
    const found = this.directories().find((row) => row.door.environmentId === door.environmentId && row.path === path);
    if (found !== undefined) {
      if (found.reader !== this.dependencies.reader()) return this.capacity('binding');
      if (found.binding === binding) return found;
      // An explicit legacy refresh may predate all panes. Its completed, ownerless observation
      // is not a source-binding claim: retire it and observe again under the pane's real binding.
      // Live owners/actions and every nonlegacy binding still require the common owner's rebase.
      const working = this.records().some((row) => row.kind === 'work' && row.directory === found.id && (!row.retired || row.action));
      if (found.binding !== 'legacy' || found.owners.length !== 0 || found.actions !== 0 || working) return this.capacity('binding');
      this.budget.remove(found.key);
    }
    const admitted = this.directories().filter((row) => row.admitted).length < MOST_WATCHED;
    if (!admitted && !waiting) return this.capacity('directories');
    const id = this.id();
    const dir: Directory = { kind: 'directory', id, key: `d:${id}`, door: { ...door }, path, binding,
      reader: this.dependencies.reader(), epoch: this.epoch, owners: [], actions: 0, admitted, version: 0, generation: this.id(),
      state: { ...EMPTY, entries: [], status: admitted ? 'loading' : 'ready', watch: admitted ? undefined : 'waiting' },
      head: null, work: null, subscription: null, install: 0, seq: null, sealed: false, unpin: null };
    if (!this.save(dir)) return this.capacity('metadata');
    return dir;
  }
  open(door: Door, path: string, options: DirectoryOpen, waiting = false): DirectoryOpened {
    return this.batch<DirectoryOpened>(() => {
      const dir = this.ensure(door, path, options.binding, waiting);
      if ('ok' in dir) return dir;
      if (!dir.admitted && !waiting && this.directories().filter((row) => row.admitted).length >= MOST_WATCHED) return this.capacity('directories');
      const parent = options.parent === undefined ? undefined : this.view(options.parent.id);
      const parentDir = parent === undefined ? undefined : this.directory(parent.directory);
      if (options.parent !== undefined && (parent?.pane !== options.pane || parentDir?.binding !== options.binding
        || doorKey(parent.door) !== doorKey(door) || parent.stale
        || !this.page(parent.selected)?.listing.entries.some((row) => row.type === 'dir' && row.path === path))) {
        this.unowned(dir); this.save(dir); return this.capacity('parent');
      }
      dir.unpin ??= this.budget.pin(dir.key);
      if (dir.unpin === null) return this.capacity('binding');
      const id = this.id();
      const view: View = { kind: 'view', id, key: `v:${id}`, directory: dir.id, door: { ...door }, pane: options.pane,
        parent: parent?.id ?? null, selected: doorKey(this.page(dir.head)?.selector.door ?? door) === doorKey(door) ? dir.head : null,
        ordinal: 1, trail: [], lost: false, expanded: [], stale: false,
        capacity: null, listener: null, snapshot: EMPTY, unpin: null, pagePin: null };
      if (!this.save(view)) { this.unowned(dir); this.save(dir); return this.capacity('metadata'); }
      view.unpin = this.budget.pin(view.key);
      if (view.selected !== null) view.pagePin = this.budget.pin(view.selected);
      const owners = dir.owners;
      dir.owners = [...owners, id];
      if (!this.save(dir)) { dir.owners = owners; this.budget.remove(view.key); this.unowned(dir); return this.capacity('metadata'); }
      this.fill();
      if (dir.admitted && dir.subscription === null) this.start(dir);
      this.changed();
      return { ok: true, lease: this.lease(id) };
    });
  }
  private lease(id: number): DirectoryLease {
    return { id, read: () => {
      const view = this.view(id); if (view === undefined) return CLOSED;
      const page = this.page(view.selected);
      // No expiry timer. A real read/action can discover expiry; cache this derived snapshot once
      // so useSyncExternalStore never sees a newly allocated object on every getSnapshot call.
      if (page !== undefined && this.expired(page) && view.snapshot.invalidated !== true) view.snapshot = this.window(view);
      return view.snapshot;
    },
      // One observer per acquisition; two consumers acquire two independently budgeted leases.
      subscribe: (listener) => { const view = this.view(id); if (view === undefined) return () => undefined;
        view.listener = listener; return () => { const current = this.view(id); if (current?.listener === listener) current.listener = null; }; },
      first: () => this.navigate(id, 'first'), next: () => this.navigate(id, 'next'), previous: () => this.navigate(id, 'previous'),
      restart: () => this.navigate(id, 'restart'), retry: () => this.retryView(id),
      expanded: (paths) => this.batch<DirectoryAdmission>(() => { const view = this.view(id); if (view === undefined) return this.capacity('binding');
        const next: View = { ...view, expanded: [...new Set(paths)] };
        if (!this.save(next)) return this.capacity('metadata'); this.changed(); return { ok: true }; }),
      release: () => this.release(id) };
  }
  private async release(id: number): Promise<void> {
    const joins: Promise<unknown>[] = [];
    this.batch(() => {
      // Authority/forget can remove a view before its effect cleanup runs. The charged old ticket
      // still carries its owner id, so that cleanup can join it without looking up a new binding.
      for (const work of this.records().filter((row): row is Work => row.kind === 'work' && row.joiners.includes(id))) {
        work.owners = work.owners.filter((owner) => owner.id !== id && owner.ancestors?.includes(id) !== true); this.save(work);
        if (work.owners.length === 0 && !work.action) { this.cancel(work); joins.push(work.promise); }
      }
      const view = this.view(id); if (view === undefined) return;
      for (const child of this.views().filter((child) => child.parent === id)) joins.push(this.release(child.id));
      const dir = this.directory(view.directory);
      view.pagePin?.(); view.unpin?.(); view.listener = null; this.budget.remove(view.key);
      if (dir !== undefined) {
        dir.owners = dir.owners.filter((owner) => owner !== id);
        this.unowned(dir); this.save(dir);
      }
      this.fill(); this.changed();
    });
    await Promise.all(joins);
  }
  private unowned(dir: Directory): void {
    if (dir.owners.length > 0 || dir.actions > 0) return;
    dir.subscription?.unsubscribe(); dir.subscription = null; dir.install += 1;
    dir.admitted = false; dir.state = { ...dir.state, watch: undefined }; dir.unpin?.(); dir.unpin = null;
  }
  private fill(): void {
    let count = this.directories().filter((row) => row.admitted).length;
    for (const dir of this.directories()) if (!dir.admitted && dir.owners.length > 0 && count < MOST_WATCHED) {
      dir.admitted = true; count += 1; this.save(dir); this.start(dir);
    }
  }
  private start(dir: Directory): void {
    const install = this.id(), id = dir.id; dir.install = install;
    dir.seq = null; dir.sealed = false; dir.state = { ...dir.state, watch: 'connecting', watchError: null };
    dir.subscription = this.dependencies.subscribe(directoryFeed(dir.door, dir.path), {
      since: () => this.directory(id)?.seq ?? null, onFrame: (frame) => this.frame(id, install, frame),
    });
    this.save(dir);
  }
  private frame(id: number, install: number, frame: FeedFrame): void {
    const dir = this.directory(id);
    if (dir?.install !== install || dir.owners.length === 0 || dir.reader !== this.dependencies.reader()
      || dir.epoch !== this.epoch) return;
    if (frame.op === 'resync' && (!dir.sealed || frame.reload === true)) {
      if (frame.reload === true) this.evictPages(dir); else this.invalidate(dir);
      dir.subscription?.unsubscribe(); dir.subscription = null; this.start(dir); this.changed(); return;
    }
    if (dir.sealed) {
      if ((frame.op === 'closed' && frame.reason !== 'unsubscribed')
        || (frame.op === 'error' && (frame.code === 'forbidden' || frame.code === 'not_found'))) {
        this.refuse(dir, null); this.changed();
      } else if (frame.op === 'event' || frame.op === 'sync') this.apply(dir.door.environmentId, dir.path, frame.data);
      return;
    }
    if (frame.op === 'subscribed') { dir.state = { ...dir.state, watch: 'connecting' }; this.save(dir); this.changed(); return; }
    if (frame.op === 'sync' || frame.op === 'event') {
      if (frame.op === 'sync') { if (dir.seq !== null && frame.seq <= dir.seq) return; dir.seq = frame.seq; }
      this.apply(dir.door.environmentId, dir.path, frame.data); return;
    }
    if (frame.op === 'closed' || frame.op === 'error') {
      if ((frame.op === 'closed' && frame.reason !== 'unsubscribed') || (frame.op === 'error' && (frame.code === 'forbidden' || frame.code === 'not_found'))) {
        this.refuse(dir, null);
      } else { dir.state = { ...dir.state, offline: true, watch: 'offline' }; this.save(dir); }
      this.changed();
    }
  }
  transport(ready: boolean): void {
    this.batch(() => { for (const dir of this.directories()) if (dir.subscription !== null && !dir.sealed) {
      dir.state = { ...dir.state, offline: true, watch: ready ? 'connecting' : 'offline' }; this.save(dir);
    } this.changed(); });
  }
  private cancel(work: Work): void {
    work.retired = true; work.controller.abort();
    const dir = this.directory(work.directory); if (dir?.work === work.key) dir.work = null;
    this.save(work);
  }
  private invalidate(dir: Directory): void {
    dir.version = this.id();
    if (dir.state.invalidated !== true) dir.generation = this.id();
    const pending = this.work(dir.work); if (pending !== undefined) this.cancel(pending);
    dir.state = { ...dir.state, invalidated: true, nextCursor: null, expiresAt: null, watch: 'stale' };
    for (const view of this.views().filter((view) => view.directory === dir.id)) {
      view.stale = true; view.lost ||= view.trail.length > 0; view.trail = []; this.save(view);
    }
    for (const row of this.records()) if (row.kind === 'checkpoint' && row.directory === dir.id) this.budget.remove(row.key);
    this.save(dir);
  }
  /** Retire published data without retiring its views or declaring a permission refusal. */
  private evictPages(dir: Directory): void {
    this.invalidate(dir);
    for (const view of this.views().filter((view) => view.directory === dir.id)) {
      view.pagePin?.(); view.pagePin = null; view.selected = null; view.expanded = []; view.trail = []; view.capacity = null; this.save(view);
    }
    for (const row of this.records()) if (row.kind === 'page' && row.directory === dir.id) this.budget.remove(row.key);
    dir.head = null; dir.state = { ...EMPTY }; this.save(dir);
  }
  private refuse(dir: Directory, error: unknown, watchError: DirectoryWatchError | null = null): void {
    this.evictPages(dir); dir.sealed = true;
    dir.state = { ...EMPTY, status: 'failed', watch: 'refused', error, watchError }; this.save(dir);
  }
  apply(environmentId: string, path: string, data: Partial<DirData> & { offline?: boolean }): void {
    this.batch(() => {
      const dir = this.directories().find((dir) => dir.door.environmentId === environmentId && dir.path === path);
      if (dir === undefined || (dir.sealed && data.error?.code !== 'authz.denied' && data.error?.code !== 'not_found')) return;
      if (data.offline === true) { dir.state = { ...dir.state, offline: true, watch: 'offline' }; this.save(dir); this.changed(); return; }
      if (data.error !== null && data.error !== undefined) {
        const failure = data.error;
        const denied = failure.code === 'authz.denied' || failure.code === 'not_found';
        const error = new ApiError({ type: `orch:error:${failure.code}`, code: failure.code, title: '', detail: failure.detail,
          status: failure.code === 'authz.denied' ? 403 : failure.code === 'not_found' ? 404 : failure.retryable ? 503 : 400, request_id: '' }, null);
        if (denied) this.refuse(dir, error, failure); else { this.invalidate(dir); dir.sealed = true;
          dir.state = { ...dir.state, status: 'failed', watch: 'failed', error, watchError: failure }; this.save(dir); }
        this.changed(); return;
      }
      const head = this.page(dir.head);
      if (data.invalidated === true || (data.reset !== true && (head === undefined || head.listing.truncated || head.listing.source !== 'live'))) {
        this.invalidate(dir); this.changed(); return;
      }
      this.invalidate(dir);
      let entries: DirEntry[];
      if (data.reset === true) entries = data.changed ?? [];
      else {
        // Only a complete head can use deltas. This transient name map is bounded by the native
        // frame/head bounds and is discarded after whole-page admission, never kept as a fallback.
        const next = new Map((head?.listing.entries ?? []).map((entry) => [entry.name, entry]));
        for (const name of data.removed ?? []) next.delete(name);
        for (const entry of data.changed ?? []) next.set(entry.name, entry);
        entries = sortEntries([...next.values()]);
      }
      const listing: DirListing = { path, mount: null, entries, source: 'live', as_of: data.as_of ?? head?.listing.as_of ?? new Date(this.now()).toISOString(),
        observed_from: data.observed_from ?? null, truncated: data.truncated ?? head?.listing.truncated ?? false,
        revision: data.revision ?? null, next_cursor: data.next_cursor ?? null, expires_at: data.expires_at ?? null };
      const selector: Selector = { incoming: null, limit: NATIVE_ROWS, source: 'live', door: dir.door };
      const owners = this.views().filter((view) => view.directory === dir.id && (this.page(view.selected)?.selector.incoming ?? null) === null);
      const adopted = this.adopt(dir, selector, listing, owners.map((view) => ({ id: view.id, mode: 'restart' as const, ordinal: 1 })), true, true);
      if (!adopted) { const reason = this.pageFailure;
        for (const view of owners) { view.capacity = this.capacity(reason); this.save(view); } }
      this.changed();
    });
  }
  private adopt(dir: Directory, selector: Selector, listing: DirListing, owners: Work['owners'], head: boolean, native = false): boolean {
    if (listing.entries.length > NATIVE_ROWS) { this.pageFailure = 'rows'; return false; }
    if (bytes(listing) > NATIVE_BYTES) { this.pageFailure = 'bytes'; return false; }
    const previous = owners.flatMap((owner) => { const view = this.view(owner.id); const page = this.page(view?.selected ?? null);
      return view === undefined ? [] : [{ view, selected: view.selected, selector: page?.selector, revision: page?.listing.revision ?? null }]; });
    // Transactional transfer: release only departing owners' page pins, plan the whole new page,
    // and restore those pins on refusal. Other panes' pins are never made evictable.
    for (const { view } of previous) { view.pagePin?.(); view.pagePin = null; }
    const id = this.id(); const page: Page = { kind: 'page', id, key: `p:${id}`, directory: dir.id,
      selector: { ...selector, door: { ...selector.door } },
      listing: Object.freeze({ ...listing, entries: Object.freeze(listing.entries.map((entry) => Object.freeze({ ...entry }))) }),
      ordinal: owners[0]?.ordinal ?? 1 };
    if (!this.save(page)) {
      this.pageFailure = this.budget.failure() ?? 'metadata';
      for (const { view, selected } of previous) if (selected !== null) view.pagePin = this.budget.pin(selected);
      return false;
    }
    const transfer = this.budget.pin(page.key);
    if (head) dir.head = page.key;
    for (const owner of owners) {
      const view = this.view(owner.id); if (view === undefined) continue;
      const old = previous.find((before) => before.view.id === owner.id)?.selected ?? null;
      if (owner.mode === 'next' && old !== null) {
        const prior = previous.find((before) => before.view.id === owner.id);
        if (prior?.selector !== undefined) { const id = this.id(); const checkpoint: Checkpoint = { kind: 'checkpoint', id, key: `c:${id}`,
          directory: dir.id, page: old, selector: prior.selector, revision: prior.revision, ordinal: view.ordinal };
          if (this.save(checkpoint)) view.trail = [...view.trail, checkpoint.key]; else view.lost = true; }
      } else if (owner.mode === 'previous') view.trail = view.trail.slice(0, -1);
      else { view.trail = []; view.lost = false; }
      view.selected = page.key; view.ordinal = owner.ordinal; view.pagePin = this.budget.pin(page.key);
      view.stale = false; view.capacity = null; this.save(view);
      this.departed(view, page.listing);
    }
    transfer?.();
    dir.state = { ...dir.state, status: dir.state.watch === 'failed' ? 'failed' : 'ready', invalidated: false,
      offline: dir.state.watch === 'offline', error: dir.state.watch === 'failed' ? dir.state.error : null,
      watch: dir.state.watch === 'stale' && listing.source === 'live' ? 'live' : dir.state.watch };
    if (native && head && listing.source === 'live' && dir.state.watch !== 'failed' && dir.state.watch !== 'offline' && dir.state.watch !== 'refused') {
      dir.state = { ...dir.state, watch: dir.subscription === null ? dir.state.watch : 'live', watchError: null };
    }
    this.save(dir); return true;
  }
  private departed(view: View, listing: ImmutableListing): void {
    view.expanded = view.expanded.filter((path) => listing.entries.some((row) => row.type === 'dir' && row.path === path));
    this.save(view);
    for (const child of this.views().filter((child) => child.parent === view.id)) {
      const childDir = this.directory(child.directory);
      if (!listing.entries.some((row) => row.type === 'dir' && row.path === childDir?.path)) void this.release(child.id);
    }
  }
  private navigate(id: number, mode: Work['owners'][number]['mode']): Promise<DirectoryNavigation> {
    return this.batch(() => Promise.resolve(this.navigateResult(id, mode)));
  }
  private navigateResult(id: number, mode: Work['owners'][number]['mode']): DirectoryNavigation | Promise<DirectoryNavigation> {
    this.budget.expire();
    const view = this.view(id), dir = view === undefined ? undefined : this.directory(view.directory);
    if (view === undefined || dir === undefined || dir.reader !== this.dependencies.reader() || dir.epoch !== this.epoch) return 'released';
    if (!dir.admitted) { view.capacity = this.capacity('directories'); this.save(view); this.changed(); return 'capacity'; }
    const page = this.page(view.selected);
    if (mode === 'first' && page !== undefined && !view.stale && !this.expired(page) && dir.state.invalidated !== true) return 'done';
    let selector: Selector = { incoming: null, limit: NATIVE_ROWS, source: 'auto', door: view.door };
    let ordinal = 1;
    if (mode === 'next') {
      if (page === undefined || view.stale || this.expired(page)
        || page.listing.next_cursor === null || page.listing.next_cursor === undefined
        || page.listing.revision === null || page.listing.revision === undefined
        || dir.state.invalidated === true) return 'restart';
      selector = { ...page.selector, door: view.door, incoming: page.listing.next_cursor, source: page.listing.source }; ordinal = view.ordinal + 1;
    }
    if (mode === 'previous') {
      const key = view.trail.at(-1), checkpoint = key === undefined ? undefined : this.budget.peek(key);
      if (checkpoint?.kind !== 'checkpoint' || checkpoint.ordinal !== view.ordinal - 1 || view.stale
        || (page !== undefined && this.expired(page))) return 'restart';
      const previous = this.page(checkpoint.page);
      if (previous?.listing.revision === checkpoint.revision && !this.expired(previous)) {
        this.batch(() => { view.pagePin?.(); view.selected = previous.key; view.pagePin = this.budget.pin(previous.key);
          view.ordinal = checkpoint.ordinal; view.trail = view.trail.slice(0, -1); this.departed(view, previous.listing); this.changed(); }); return 'done';
      }
      // A lost first page cannot be refetched with null without inventing the old snapshot.
      if (checkpoint.selector.incoming === null) return 'restart';
      selector = { ...checkpoint.selector, door: view.door }; ordinal = checkpoint.ordinal;
    }
    return this.request(dir, selector, { id, mode, ordinal }, false);
  }
  private request(dir: Directory, selector: Selector, owner: Work['owners'][number] | null, action: boolean): Promise<DirectoryNavigation> {
    if (owner !== null) {
      const ancestors: number[] = [];
      let parent = this.view(owner.id)?.parent;
      // Only already admitted ancestors, each older than its child; no caller-supplied ancestry.
      while (parent !== null && parent !== undefined) { ancestors.push(parent); parent = this.view(parent)?.parent; }
      owner = { ...owner, ancestors };
    }
    const current = this.work(dir.work);
    if (current !== undefined && !current.retired) {
      if (JSON.stringify(current.selector) !== JSON.stringify(selector)) return Promise.resolve('busy');
      const owners = current.owners, joiners = current.joiners;
      if (owner !== null && !owners.some((row) => row.id === owner.id)) current.owners = [...owners, owner];
      if (owner !== null) current.joiners = [...new Set([...joiners, owner.id, ...(owner.ancestors ?? [])])];
      if (!this.save(current)) {
        current.owners = owners; current.joiners = joiners; if (owner !== null) this.recordCapacity(owner.id); return Promise.resolve('capacity');
      }
      if (action && !current.action) { current.action = true; dir.actions += 1; this.save(dir); }
      this.changed(); return current.promise;
    }
    const id = this.id(), controller = new AbortController();
    const base = owner === null ? this.page(dir.head) : this.page(this.view(owner.id)?.selected ?? null);
    const baseRevision = base?.listing.revision ?? null;
    const work: Work = { kind: 'work', id, key: `w:${id}`, directory: dir.id, selector, version: dir.version,
      path: dir.path, binding: dir.binding,
      epoch: this.epoch, reader: this.dependencies.reader(), expected: selector.incoming === null || base === undefined || baseRevision === null ? null
        : { source: base.listing.source, revision: baseRevision, asOf: base.listing.as_of, from: base.listing.observed_from ?? null },
      owners: owner === null ? [] : [owner], joiners: owner === null ? [] : [owner.id, ...(owner.ancestors ?? [])],
      action, retired: false, controller, promise: Promise.resolve('released'), unpin: null };
    dir.unpin ??= this.budget.pin(dir.key);
    if (dir.unpin === null) return Promise.resolve('released');
    if (!this.save(work)) { this.unowned(dir); if (owner !== null) this.recordCapacity(owner.id); return Promise.resolve('capacity'); }
    work.unpin = this.budget.pin(work.key); dir.work = work.key; dir.actions += Number(action);
    dir.unpin ??= this.budget.pin(dir.key); this.save(dir);
    // The joined continuation retains only this charged ticket, never an evicted Directory/View.
    const promise = this.run(work);
    work.promise = promise; this.save(work); this.changed(); return promise;
  }
  private recordCapacity(id: number): void {
    const view = this.view(id); if (view === undefined) return;
    view.capacity = this.capacity(this.budget.failure() ?? 'metadata');
    this.save(view); this.changed();
  }
  private current(work: Work): Directory | undefined {
    const dir = this.directory(work.directory);
    if (dir === undefined) return undefined;
    return work.retired || work.controller.signal.aborted || this.epoch !== work.epoch || work.reader !== this.dependencies.reader()
      || dir.version !== work.version || dir.binding !== work.binding ? undefined : dir;
  }
  private run(work: Work): Promise<DirectoryNavigation> {
    return this.reads.run(work.controller.signal, () => this.dependencies.fetch(work.selector.door, work.path,
      { source: work.selector.source, ...(work.selector.incoming === null ? {} : { cursor: work.selector.incoming }),
        limit: work.selector.limit, signal: work.controller.signal })).then(
      (listing) => this.received(work, listing), (error: unknown) => this.failed(work, error),
    ).finally(() => this.finished(work));
  }
  private received(work: Work, listing: DirListing): DirectoryNavigation {
    return this.batch(() => {
      const dir = this.current(work); if (dir === undefined) return 'released';
      if (listing.path !== work.path || (work.selector.source !== 'auto' && listing.source !== work.selector.source)
        || (work.selector.incoming !== null && listing.next_cursor === work.selector.incoming)
        || (work.expected !== null && (listing.source !== work.expected.source
        || listing.revision !== work.expected.revision || listing.as_of !== work.expected.asOf
        || (listing.observed_from ?? null) !== work.expected.from))) {
        this.invalidate(dir); this.changed(); return 'restart';
      }
      const head = work.selector.incoming === null;
      const prior = this.page(dir.head)?.listing;
      if (head && prior !== undefined && (prior.source !== listing.source || prior.revision !== listing.revision
        || prior.as_of !== listing.as_of || prior.observed_from !== listing.observed_from)) {
        // A fresh first observation cannot rebase another pane's later-page cursor.
        // Keep this request alive, fence every other old selector, and select only its own owners.
        dir.generation = this.id();
        dir.state = { ...dir.state, invalidated: true, nextCursor: null,
          watch: dir.state.watch === 'live' ? 'stale' : dir.state.watch };
        for (const view of this.views().filter((view) => view.directory === dir.id)) {
          view.stale = true; view.lost ||= view.trail.length > 0; view.trail = []; this.save(view);
        }
        for (const row of this.records()) if (row.kind === 'checkpoint' && row.directory === dir.id) this.budget.remove(row.key);
      }
      const adopted = this.adopt(dir, work.selector, listing, work.owners, head);
      if (!adopted) { const reason = this.pageFailure;
        for (const held of work.owners) { const view = this.view(held.id); if (view !== undefined) {
          view.capacity = this.capacity(reason); this.save(view); } } this.save(dir); }
      this.changed(); return adopted ? 'done' : 'capacity';
    });
  }
  private failed(work: Work, failure: unknown): DirectoryNavigation {
    return this.batch(() => {
      const dir = this.current(work); if (dir === undefined) return 'released';
      const error = failure instanceof ApiError ? failure
        : failure instanceof NetworkError ? new NetworkError(failure.failure, failure.status)
          : new Error(failure instanceof Error ? failure.message : typeof failure === 'string' ? failure : 'Directory request failed');
      if (error instanceof ApiError && (error.is('authz.denied') || error.is('not_found'))) this.refuse(dir, error);
      else if (error instanceof ApiError && error.is('conflict.stale_revision')) this.invalidate(dir);
      else { dir.state = { ...dir.state, status: 'failed', error }; this.save(dir); }
      this.changed(); return error instanceof ApiError && error.is('conflict.stale_revision') ? 'restart' : 'done';
    });
  }
  private finished(work: Work): void {
    this.batch(() => {
      const dir = this.directory(work.directory);
      work.unpin?.(); this.budget.remove(work.key);
      if (dir !== undefined) {
        if (dir.work === work.key) dir.work = null;
        dir.actions -= Number(work.action); this.unowned(dir);
        // A cancelled visible-only completion is not a new owner/use of an old directory.
        // In particular it must not renew an unowned record's lazy retention or resurrect it.
        if (dir.owners.length > 0 || dir.actions > 0) this.save(dir);
      }
      this.fill(); this.changed();
    });
  }
  private retryView(id: number): Promise<DirectoryNavigation> {
    const view = this.view(id), dir = view === undefined ? undefined : this.directory(view.directory);
    if (view === undefined || dir === undefined) return Promise.resolve('released');
    if (dir.state.watch === 'failed' || dir.state.watch === 'refused') { dir.subscription?.unsubscribe(); dir.subscription = null; this.start(dir); }
    if ((this.page(view.selected)?.selector.incoming ?? null) !== null && view.stale) { this.changed(); return Promise.resolve('restart'); }
    return this.navigate(id, 'restart');
  }
  load(door: Door, path: string, options: { source?: Source; visible?: boolean } = {}): Promise<void> {
    return this.batch(() => this.loadResult(door, path, options));
  }
  private loadResult(door: Door, path: string, options: { source?: Source; visible?: boolean }): Promise<void> {
    this.budget.expire();
    const found = this.directories().find((row) => row.door.environmentId === door.environmentId && row.path === path);
    if (options.visible === true && (found?.owners.length ?? 0) === 0) return Promise.resolve();
    const dir = found ?? this.ensure(door, path, 'legacy', false);
    if ('ok' in dir) return Promise.resolve();
    if (!dir.admitted) {
      if (this.directories().filter((row) => row.admitted).length >= MOST_WATCHED) return Promise.resolve();
      dir.admitted = true; this.save(dir);
    }
    const selector = { door: { ...door }, incoming: null, source: options.source ?? 'auto', limit: NATIVE_ROWS };
    const owners = this.views().filter((view) => view.directory === dir.id && doorKey(view.door) === doorKey(door)
      && (this.page(view.selected)?.selector.incoming ?? null) === null);
    const first = owners[0];
    const result = this.request(dir, selector, first === undefined ? null : { id: first.id, mode: 'restart', ordinal: 1 }, options.visible !== true);
    for (const view of owners.slice(1)) void this.request(dir, selector, { id: view.id, mode: 'restart', ordinal: 1 }, options.visible !== true);
    return result.then(() => undefined);
  }
  async retry(door: Door, path: string, options: { source?: Source } = {}): Promise<void> {
    const dir = this.directories().find((row) => row.door.environmentId === door.environmentId && row.path === path);
    if (dir !== undefined && (dir.state.watch === 'failed' || dir.state.watch === 'refused')) {
      dir.subscription?.unsubscribe(); dir.subscription = null; this.start(dir);
    }
    await this.load(door, path, options);
  }
  watch(door: Door, path: string): () => void {
    const opened = this.open(door, path, { pane: 'legacy', binding: 'legacy' }, true);
    if (!opened.ok) return () => undefined;
    return () => { void opened.lease.release(); };
  }
  forget(environmentId: string): void {
    this.batch(() => { for (const dir of this.directories()) if (dir.door.environmentId === environmentId) this.budget.remove(dir.key);
      this.fill(); this.changed(); });
  }
  clear(): void {
    this.epoch += 1;
    this.batch(() => { for (const row of this.records()) if (row.kind !== 'work') this.budget.remove(row.key);
      for (const row of this.records()) if (row.kind === 'work') this.cancel(row); this.changed(); });
  }
  private retired(_key: string, row: Record): void {
    if (row.kind === 'directory') {
      row.subscription?.unsubscribe(); row.subscription = null; row.install += 1;
      for (const work of this.records()) if (work.kind === 'work' && work.directory === row.id) this.cancel(work);
      for (const held of this.records()) if (held.kind !== 'work' && held.kind !== 'directory' && held.directory === row.id) this.budget.remove(held.key);
      row.owners = []; row.head = null; row.work = null;
    } else if (row.kind === 'view') { row.pagePin?.(); row.unpin?.(); row.listener = null; }
    else if (row.kind === 'page') {
      const dir = this.directory(row.directory); if (dir?.head === row.key) dir.head = null;
    } else if (row.kind === 'checkpoint') for (const view of this.views()) if (view.trail.includes(row.key)) {
      view.trail = view.trail.filter((key) => key !== row.key); view.lost = true;
    }
    this.dirty = true;
  }
}
