/** Load-then-subscribe, sequence repair and row replacement.
 *
 * A feed that is kept in order promises this: after it is subscribed from a sequence, every row
 * that changed after that sequence arrives, in order, none skipped. Each frame says which frame came
 * before it. So the page holds, for each such feed, the sequence it has come to, and:
 *
 * - loads the whole state over HTTP first, and subscribes from the sequence of that answer;
 * - takes a frame whose "before" is where the page stands, drops one that brings nothing new, and
 *   on any other asks HTTP for what changed since where it stands, before going on;
 * - loads everything again when the server says that the gap is too large to fill;
 * - after a reconnect, subscribes again from where it stands.
 *
 * What the rows are, and where they are kept, is the feed's own business: this module is given the
 * request that loads them and the store that takes them.
 */
import { retryPause, retryRead } from '@/lib/requests';
import type { FeedFrame, RealtimeSocket, Subscription } from '@/realtime/socket';

/** An answer of a feed's HTTP request. */
export interface Loaded<Data> {
  /** The sequence the answer is as of. */
  seq: number;
  /** False when more changed than the request can say: the whole state is loaded instead. */
  complete: boolean;
  data: Data;
}

/** Optional selectors and an admission fence captured with one socket subscription. */
export interface FeedScope {
  continuationId?: string;
  projectionRevision?: number;
  accept: (frame: FeedFrame) => 'accept' | 'ignore' | 'reload' | 'refuse';
}

/** What one durable feed is made of. */
export interface DurableFeed<Data> {
  /** The name of the feed on the socket: `org:<id>`. */
  feed: string;
  /** Other feeds omit this and retain their original behavior. */
  scope?: () => FeedScope;
  /** Retire local positioned reads before a fresh bounded opening. */
  resetting?: () => void;
  /** A native full resync also retires dependent projections not carried by this feed. */
  invalidated?: () => void;
  /** An authoritative bootstrap already loaded this cursor; subscribe without reading it twice. */
  initial?: () => Loaded<Data> | null;
  /** Asks HTTP for the whole state (`since` is `null`) or for what changed after `since`. */
  load: (since: number | null, signal: AbortSignal) => Promise<Loaded<Data>>;
  /** Takes a whole state in place of what was held. */
  reset: (data: Data, seq: number) => void;
  /** Takes rows that changed; each replaces what is held only when it is newer. */
  apply: (data: Data, seq: number) => void;
  /** Reads the rows of a frame's `data`. */
  read: (data: Record<string, unknown>) => Data;
  /** The feed could not be loaded; what is held stays. */
  failed?: (error: unknown) => void;
  /** Its scoped protocol cannot identify a valid read; distinct from an ordinary transport failure. */
  refused?: (error: unknown) => void;
  /** The server ended the subscription: the reader may no longer see the thing, or it is gone. */
  closed?: (reason: 'revoked' | 'deleted') => void;
  /** Frames that carry no rows: growing text, who is looking, events. */
  other?: (frame: Extract<FeedFrame, { op: 'delta' | 'presence' | 'event' }>) => void;
  /** The server took the subscription: what only a subscribed feed can be told (that it is on screen) can be said now. */
  subscribed?: (frame: Extract<FeedFrame, { op: 'subscribed' }>) => void;
}

export interface FeedFollower {
  /** The sequence the page has come to; `null` before the first load. */
  lastSeq: () => number | null;
  /** Loads the feed again from nothing: after a failure, when its reader asks. */
  reload: () => void;
  /** Asks for what changed since the page's sequence: after an act of its own, where nothing else says that it changed. */
  refresh: () => void;
  /** Stops following. What the store holds stays. */
  stop: () => void;
}

/**
 * Follows a durable feed: loads it, subscribes, and keeps the store equal to the server's rows
 * whatever frames are lost on the way.
 */
export function followFeed<Data>(socket: RealtimeSocket, feed: DurableFeed<Data>): FeedFollower {
  let lastSeq: number | null = null;
  let subscription: Subscription | null = null;
  let stoppedNow = false;
  let generation = 0;
  let socketGeneration = 0;
  let abort = new AbortController();
  let busy: Promise<void> = Promise.resolve();
  const current = (version: number): boolean => !stoppedNow && version === generation;

  const run = (work: (version: number) => Promise<void>): void => {
    const version = generation;
    busy = busy.then(async () => {
      if (!current(version)) return;
      try { await work(version); }
      catch (error) { if (current(version)) feed.failed?.(error); }
    });
  };

  /** A former authority's work cannot publish rows, errors or cursors into its replacement. */
  function retireLoads(): void {
    generation += 1;
    abort.abort();
    abort = new AbortController();
    lastSeq = null;
    // A cancelled transport may ignore abort. Its completion is fenced, and cannot hold up the new reader.
    busy = Promise.resolve();
  }

  async function loadWhole(version: number): Promise<void> {
    const loaded = await feed.load(null, abort.signal);
    if (!current(version)) return;
    if (!loaded.complete) throw new Error('The full feed opening was incomplete');
    feed.reset(loaded.data, loaded.seq);
    lastSeq = loaded.seq;
  }

  async function repair(version: number): Promise<void> {
    if (lastSeq === null) {
      await loadWhole(version);
      if (current(version) && feed.scope !== undefined) subscribe();
      return;
    }
    const loaded = await feed.load(lastSeq, abort.signal);
    if (!current(version)) return;
    if (!loaded.complete) {
      await loadWhole(version);
      if (current(version) && feed.scope !== undefined) subscribe();
      return;
    }
    feed.apply(loaded.data, loaded.seq);
    lastSeq = Math.max(lastSeq, loaded.seq);
  }

  function subscribe(): void {
    const version = ++socketGeneration;
    subscription?.unsubscribe();
    const scope = feed.scope?.();
    const accepts = (frame: FeedFrame): boolean => {
      if (stoppedNow || version !== socketGeneration) return false;
      const decision = scope?.accept(frame) ?? 'accept';
      if (decision === 'reload') restart();
      else if (decision === 'refuse') {
        socketGeneration += 1;
        subscription?.unsubscribe(); subscription = null;
        retireLoads();
        const refused = feed.refused ?? feed.failed;
        refused?.(new Error('The live reply did not identify the selected history'));
      }
      return decision === 'accept';
    };
    subscription = socket.subscribe(feed.feed, { since: () => lastSeq,
      ...(scope?.continuationId === undefined ? {} : { continuationId: scope.continuationId }),
      ...(scope?.projectionRevision === undefined ? {} : { projectionRevision: scope.projectionRevision }),
      onFrame: (frame) => { if (accepts(frame)) onFrame(frame, () => accepts(frame)); },
    });
  }

  function onFrame(frame: FeedFrame, accepts: () => boolean): void {
    switch (frame.op) {
      case 'subscribed':
        feed.subscribed?.(frame);
        if (frame.mode === 'reload') restart();
        return;
      case 'sync':
        run(async (version) => {
          if (!accepts()) return;
          if (lastSeq !== null && frame.seq <= lastSeq) return;
          if (lastSeq === null || frame.prev > lastSeq) {
            await repair(version);
            if (!current(version) || !accepts() || (lastSeq !== null && frame.seq <= lastSeq)) return;
          }
          feed.apply(feed.read(frame.data), frame.seq);
          lastSeq = frame.seq;
        });
        return;
      case 'resync':
        if (frame.reload === true) {
          feed.invalidated?.();
          restart();
        } else run(repair);
        return;
      case 'closed':
        subscription = null;
        if (frame.reason !== 'unsubscribed') { retireLoads(); feed.closed?.(frame.reason); }
        return;
      case 'error':
        if (frame.code === 'forbidden' || frame.code === 'not_found') {
          // The socket may retry this wanted subscription after new proof. Any old HTTP load is retired now.
          retireLoads();
          feed.closed?.(frame.code === 'forbidden' ? 'revoked' : 'deleted');
        } else {
          if (frame.code !== 'too_many_subscriptions') subscription = null;
          feed.failed?.(new Error(`${feed.feed}: ${frame.code}: ${frame.detail}`));
        }
        return;
      default:
        feed.other?.(frame);
    }
  }

  function restart(): void {
    if (stoppedNow) return;
    socketGeneration += 1;
    subscription?.unsubscribe(); subscription = null;
    retireLoads();
    feed.resetting?.();
    begin();
  }

  function begin(useInitial = false): void {
    run(async (version) => {
      const initial = useInitial ? feed.initial?.() : null;
      if (initial?.complete === true) { feed.apply(initial.data, initial.seq); lastSeq = initial.seq; }
      else await loadWhole(version);
      if (current(version)) subscribe();
    });
  }
  begin(true);

  return {
    lastSeq: () => lastSeq,
    reload: restart,
    refresh() { if (!stoppedNow) run(repair); },
    stop() {
      stoppedNow = true;
      socketGeneration += 1;
      retireLoads();
      subscription?.unsubscribe(); subscription = null;
    },
  };
}

/**
 * Asks again when a load failed for the network, as every read does: three times, after one, two and
 * four seconds. An answer of the API is an answer, and is not asked for again.
 */
export async function loadWithRetries<Result>(load: () => Promise<Result>, signal: AbortSignal): Promise<Result> {
  for (let failures = 0; ; failures += 1) {
    try {
      return await load();
    } catch (error) {
      if (signal.aborted || !retryRead(failures, error)) throw error;
      await new Promise((resolve) => setTimeout(resolve, retryPause(failures)));
    }
  }
}

// ---------------------------------------------------------------- feeds shared by whoever shows them

/** A feed stays subscribed this long after the last view that showed it left: going back and forth costs nothing. */
export const UNSUBSCRIBE_AFTER_MS = 10_000;
/** What a feed delivered is kept this long after that, and then forgotten. */
export const EVICT_AFTER_MS = 5 * 60_000;

interface Shared {
  follower: FeedFollower | null;
  users: number;
  cacheUsers: number;
  /** One explicitly demanded HTTP opening shares this cache entry, without a follower/socket. */
  reading?: AbortController;
  /** Authority reset may preserve a non-authority read locator while retiring all content. */
  rebase?: () => void;
  evict: () => void;
  leaving?: ReturnType<typeof setTimeout>;
  evicting?: ReturnType<typeof setTimeout>;
}

/**
 * The feeds that are followed at the moment, each once however many views show it. A feed is
 * followed while any view uses it, a little longer after the last one left, and what it delivered
 * is kept a while beyond that.
 */
export class FeedRegistry {
  private readonly shared = new Map<string, Shared>();

  /** Keep an HTTP-only thread read in this same cache, without subscribing or loading it.
   * Its unused retention is the existing five-minute cache window, not a second lifetime map. */
  retain(key: string, evict: () => void, rebase?: () => void): void {
    const existing = this.shared.get(key);
    if (existing !== undefined && rebase !== undefined) existing.rebase = rebase;
    if (existing !== undefined && (existing.follower !== null || existing.cacheUsers > 0)) return;
    const entry: Shared = existing ?? { follower: null, users: 0, cacheUsers: 0, evict, rebase };
    clearTimeout(entry.evicting);
    entry.evicting = setTimeout(() => {
      if (this.shared.get(key) !== entry || entry.users !== 0 || entry.cacheUsers !== 0 || entry.follower !== null) return;
      this.shared.delete(key);
      entry.evict();
    }, EVICT_AFTER_MS);
    this.shared.set(key, entry);
  }

  /** A visible nonstreaming reader or an in-flight HTTP body owns the same cache, without a sub. */
  hold(key: string, evict: () => void, rebase?: () => void): () => void {
    const entry: Shared = this.shared.get(key) ?? { follower: null, users: 0, cacheUsers: 0, evict, rebase };
    if (rebase !== undefined) entry.rebase = rebase;
    clearTimeout(entry.evicting); entry.evicting = undefined;
    entry.cacheUsers += 1;
    this.shared.set(key, entry);
    let released = false;
    return () => {
      if (released) return; released = true;
      entry.cacheUsers -= 1;
      if (this.shared.get(key) !== entry) return;
      if (entry.cacheUsers !== 0 || entry.users !== 0 || entry.leaving !== undefined) return;
      entry.evicting = setTimeout(() => {
        if (this.shared.get(key) !== entry || entry.cacheUsers !== 0 || entry.users !== 0) return;
        this.shared.delete(key); entry.follower?.stop(); entry.evict();
      }, EVICT_AFTER_MS);
    };
  }

  /**
   * Uses a feed. `make` is called when nobody follows it yet; `evict` forgets what it delivered.
   * What is returned says that this user is done with it.
   */
  use(key: string, make: () => FeedFollower, evict: () => void, rebase?: () => void): () => void {
    let entry = this.shared.get(key);
    if (entry === undefined) {
      entry = { follower: make(), users: 0, cacheUsers: 0, evict, rebase };
      this.shared.set(key, entry);
    } else if (entry.follower === null || entry.evicting !== undefined && entry.leaving === undefined && entry.users === 0) {
      // Its rows are still held but nobody follows it any more: follow it again from nothing.
      clearTimeout(entry.evicting);
      entry.evicting = undefined;
      entry.reading?.abort(); entry.reading = undefined;
      entry.follower = make();
    }
    if (rebase !== undefined) entry.rebase = rebase;
    clearTimeout(entry.leaving);
    clearTimeout(entry.evicting);
    entry.leaving = undefined;
    entry.evicting = undefined;
    entry.users += 1;

    const held = entry;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      held.users -= 1;
      if (this.shared.get(key) !== held || held.users > 0) return;
      held.leaving = setTimeout(() => {
        if (this.shared.get(key) !== held || held.users > 0) return;
        held.leaving = undefined;
        held.follower?.stop();
        held.follower = null;
        if (held.cacheUsers > 0) return;
        held.evicting = setTimeout(() => {
          if (this.shared.get(key) !== held || held.users > 0 || held.cacheUsers > 0) return;
          this.shared.delete(key);
          held.evict();
        }, EVICT_AFTER_MS);
      }, UNSUBSCRIBE_AFTER_MS);
    };
  }

  /** Reload through its follower, or one bounded HTTP read for an actual held-only demand.
   * A retained idle cache never starts a read. Replacement/clear owns cancellation in this entry. */
  reload(key: string, whenHeld?: {
    load: (signal: AbortSignal) => Promise<void>;
    failed: (error: unknown) => void;
  }): void {
    const entry = this.shared.get(key);
    if (entry === undefined) return;
    entry.reading?.abort(); entry.reading = undefined;
    if (entry.follower !== null) { entry.follower.reload(); return; }
    if (entry.cacheUsers === 0 || whenHeld === undefined) return;
    const reading = new AbortController();
    entry.reading = reading;
    const owns = () => this.shared.get(key) === entry && entry.reading === reading && !reading.signal.aborted;
    const finish = () => { if (entry.reading === reading) entry.reading = undefined; };
    void Promise.resolve().then(async () => {
      if (owns() && entry.cacheUsers > 0) await whenHeld.load(reading.signal);
    }).then(finish, (error: unknown) => {
      if (owns()) whenHeld.failed(error);
      finish();
    });
  }

  /** Brings a feed up to date by asking, as a person does after their own act. */
  refresh(key: string): void {
    this.shared.get(key)?.follower?.refresh();
  }

  /** A destroyed local owner retires its task/follower and cache atomically with respect to later releases. */
  forget(key: string): boolean {
    const entry = this.shared.get(key);
    if (entry === undefined) return false;
    this.shared.delete(key);
    clearTimeout(entry.leaving); clearTimeout(entry.evicting);
    entry.reading?.abort(); entry.reading = undefined;
    entry.follower?.stop(); entry.evict();
    return true;
  }

  /** A changed authority keeps subscriptions, but none of their former content or incremental cursor. */
  rebase(): void {
    for (const entry of this.shared.values()) {
      entry.reading?.abort(); entry.reading = undefined;
      const reset = entry.rebase ?? entry.evict;
      reset();
      if (entry.users > 0) entry.follower?.reload();
      else { entry.follower?.stop(); entry.follower = null; }
    }
  }

  /** Stops following everything and forgets it: whoever was signed in has left. */
  clear(): void {
    for (const entry of this.shared.values()) {
      entry.reading?.abort(); entry.reading = undefined;
      clearTimeout(entry.leaving);
      clearTimeout(entry.evicting);
      entry.follower?.stop();
      entry.evict();
    }
    this.shared.clear();
  }
}
