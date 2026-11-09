/** Personal events have no replay. Join first; only then take the HTTP baseline. */
import type { DurableFeed, FeedFollower } from '@/realtime/sync';
import type { RealtimeSocket } from '@/realtime/socket';

let baseline: (() => Promise<void>) | undefined;
export function personalBaselineReader(read: () => Promise<void>): void { baseline = read; }
export async function retakePersonalBaseline(): Promise<boolean> { if (baseline === undefined) return false; await baseline(); return true; }

const heldEvents = new Set<symbol>();
let events: (() => void)[] = [];
/** Apply events delivered during a baseline after publishing that baseline. */
export function holdPersonalEvents(): () => void {
  const token = Symbol(); heldEvents.add(token);
  return () => {
    if (!heldEvents.delete(token) || heldEvents.size > 0) return;
    const pending = events; events = [];
    for (const apply of pending) apply();
  };
}
export function dispatchPersonalEvent(apply: () => void): void {
  if (heldEvents.size > 0) events.push(apply); else apply();
}
export function retirePersonalEvents(): void { heldEvents.clear(); events = []; }

function personalSourceError(error: unknown): Error {
  return error instanceof Error ? error : new Error('The personal live source is unavailable', { cause: error });
}

/** An unavailable live transport must not take away authorized HTTP views.
 * A later accepted source still needs a new post-admission baseline.
 */
export function admittedOrOffline(ready: Promise<void>, status: RealtimeSocket['status']): Promise<boolean> {
  const unavailable = () => status.getState().phase === 'waiting' || status.getState().phase === 'idle';
  if (unavailable()) return Promise.resolve(false);
  return new Promise<boolean>((resolve, reject) => {
    let settled = false;
    let leave: () => void = () => undefined;
    const finish = (accepted: boolean) => { if (settled) return; settled = true; leave(); resolve(accepted); };
    leave = status.subscribe(() => { if (unavailable()) finish(false); });
    void ready.then(() => { finish(true); }, (error: unknown) => { if (!settled) { settled = true; leave(); reject(personalSourceError(error)); } });
    if (unavailable()) finish(false);
  });
}

export function followPersonalSource<T>(socket: RealtimeSocket, feed: DurableFeed<T>, changed: () => void): FeedFollower & { readonly ready: Promise<void> } {
  let seq: number | null = null;
  let closed = false;
  let generation = 0;
  let abort = new AbortController();
  let subscription: ReturnType<RealtimeSocket['subscribe']> | null = null;
  let completed = false;
  let accept!: () => void;
  let refuse!: (error: unknown) => void;
  let admission!: Promise<void>;
  const waiting = () => {
    completed = false;
    admission = new Promise<void>((resolve, reject) => {
      accept = () => { completed = true; resolve(); };
      refuse = (error) => { completed = true; reject(personalSourceError(error)); };
    });
    void admission.catch(() => undefined);
  };
  waiting();
  let repairing: { generation: number; promise: Promise<void> } | undefined;
  function repair(rejoin = false): Promise<void> {
    if (repairing?.generation === generation) return repairing.promise;
    const version = generation;
    const promise = (async () => {
      try {
        let whole = seq === null;
        let loaded = await feed.load(seq, abort.signal);
        if (!loaded.complete) { whole = true; loaded = await feed.load(null, abort.signal); }
        if (closed || generation !== version) return;
        if (whole) feed.reset(loaded.data, loaded.seq); else feed.apply(loaded.data, loaded.seq);
        seq = loaded.seq;
        if (rejoin) join();
      } catch (error) { if (!closed && generation === version) { refuse(error); feed.failed?.(error); } }
    })();
    const flight = { generation: version, promise }; repairing = flight;
    void promise.finally(() => { if (repairing === flight) repairing = undefined; });
    return promise;
  }
  function join() {
    subscription?.unsubscribe();
    subscription = socket.subscribe(feed.feed, {
      since: () => seq,
      onSubscribe: () => {
        generation += 1; abort.abort(); abort = new AbortController();
        if (completed) waiting();
        changed();
      },
      onFrame: (frame) => {
        if (closed) return;
        if (frame.op === 'subscribed') {
          if (frame.mode === 'reload') { seq = null; void repair(true); return; }
          seq = frame.seq ?? seq;
          changed(); accept(); feed.subscribed?.(frame);
        } else if (frame.op === 'sync') {
          if (seq !== null && frame.seq <= seq) return;
          if (seq !== null && frame.prev > seq) { void repair(); return; }
          feed.apply(feed.read(frame.data), frame.seq); seq = frame.seq;
        } else if (frame.op === 'resync') {
          if (frame.reload === true) { seq = null; feed.invalidated?.(); }
          void repair(frame.reload === true);
        } else if (frame.op === 'closed' || frame.op === 'error') {
          const error = new Error('The personal live source is unavailable');
          refuse(error);
          if (frame.op === 'closed' && frame.reason !== 'unsubscribed') feed.closed?.(frame.reason);
          else feed.failed?.(error);
        } else feed.other?.(frame);
      },
    });
  }
  join();
  return {
    get ready() { return admission; },
    lastSeq: () => seq,
    refresh: () => { void repair(); },
    reload: () => { seq = null; void repair(true); },
    stop: () => { closed = true; generation += 1; abort.abort(); subscription?.unsubscribe(); refuse(new Error('The personal live source was replaced')); },
  };
}
