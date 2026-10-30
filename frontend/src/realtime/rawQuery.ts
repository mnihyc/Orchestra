/** ACK-gated raw observations. Native source identity and local read ownership stay separate. */
import { CancelledError, infiniteQueryOptions, type InfiniteData, type Query, type QueryClient, type QueryKey } from '@tanstack/react-query';
import { api, client, ok } from '@/api/client';
import { assertThreadRead, isCurrentThreadRead, readInThread, receiveThreadRead, threadQueryRead,
  threadReadSelector, type ThreadReadCapture } from '@/api/threadRead';
import type { RawEvent, ThreadContinuationProjection, ThreadRawListReply } from '@/generated/api-types';
import type { FeedFrame, RealtimeSocket, Subscription } from '@/realtime/socket';
import { holdThreadRead, threadViewOf, threadWriter, useThreadStore } from '@/realtime/stores/thread';
import { sameThreadProjection, stampThreadReceipt, threadReadCacheKey, threadReadChanged, threadReadTarget,
  readThreadProjection, sameThreadRead, type ThreadReadRef, type ThreadReadTarget } from '@/realtime/threadScope';

/** The existing client page bound also caps staging before an HTTP page can be committed. */
const RAW_PAGE = 100;
type Pages = InfiniteData<ThreadRawListReply, number | null>;

class RawDescriptor {
  readonly ref: ThreadReadRef;
  readonly segment: string;
  readonly captured: ThreadReadCapture | null;
  owner: { readonly join: RawJoin; readonly query: Query } | null = null;
  constructor(target: ThreadReadTarget, segment: string) {
    this.ref = threadReadTarget(target);
    this.segment = segment;
    const scope = threadViewOf(this.ref).read;
    this.captured = scope?.ready === true && scope.projection !== null && !scope.provisional
      ? { ref: this.ref, threadId: this.ref.threadId, scope } : null;
  }
}
/** Explicit metadata, rather than a query-key prefix, admits a query to the raw transport. */
interface RawQueryMeta extends Record<string, unknown> { scopedRaw: RawDescriptor }

/** Retains a native refusal's cause without inventing an original-history identity. */
export class RawSourceError extends Error {
  readonly code: string;
  constructor(code: string, detail: string) { super(detail); this.name = 'RawSourceError'; this.code = code; }
}
const protocolError = () => new RawSourceError('bad_feed', 'The raw subscription did not identify its captured source');
const installed = new WeakMap<QueryClient, RawJoin>();
/** Page identity survives the awaits between queryFn, infinite aggregation and Query.setData. */
const publications = new WeakMap<object, Publication>();
/** Concurrent infinite fetches can retain an older published page no longer in the current head. */
const publishedPages = new WeakMap<object, PublishedPage>();
/** A retained query survives an observer hide/remount; its authority changes only on reload. */
const publicationAuthorities = new WeakMap<Query, object>();
function publicationAuthority(query: Query): object {
  const held = publicationAuthorities.get(query);
  if (held !== undefined) return held;
  const authority = {}; publicationAuthorities.set(query, authority); return authority;
}
const retiredPublication = () => new CancelledError({ silent: true, revert: false });

/** Retire a cancelled attempt even if an already-dispatched HTTP implementation settles late.
 * The request still receives the same abort signal; its late completion has a rejection handler. */
function interrupt<Value>(pending: Promise<Value>, signal: AbortSignal): Promise<Value> {
  return new Promise((resolve, reject) => {
    const abort = () => { signal.removeEventListener('abort', abort); reject(threadReadChanged()); };
    signal.addEventListener('abort', abort, { once: true });
    void pending.then((value) => { signal.removeEventListener('abort', abort); resolve(value); },
      (error: unknown) => { signal.removeEventListener('abort', abort); reject(error instanceof Error ? error : protocolError()); });
    if (signal.aborted) abort();
  });
}

function descriptor(query: Query): RawDescriptor | null {
  const value: unknown = query.meta?.scopedRaw;
  return value instanceof RawDescriptor ? value : null;
}
function validEvent(value: unknown): value is RawEvent {
  return typeof value === 'object' && value !== null && 'n' in value && typeof value.n === 'number'
    && Number.isSafeInteger(value.n) && value.n > 0 && 'at' in value && typeof value.at === 'string'
    && 'direction' in value && (value.direction === 'to_harness' || value.direction === 'from_harness') && 'payload' in value;
}
function validPage(value: unknown): value is ThreadRawListReply {
  if (typeof value !== 'object' || value === null || !('events' in value) || !Array.isArray(value.events)
    || value.events.length > RAW_PAGE || !('has_older' in value) || typeof value.has_older !== 'boolean') return false;
  const events: unknown[] = value.events;
  let previous = Infinity;
  for (const event of events) {
    if (!validEvent(event) || event.n >= previous) return false;
    previous = event.n;
  }
  return !value.has_older || events.length > 0;
}
function pagesOf(value: unknown): Pages | null {
  if (typeof value !== 'object' || value === null || !('pages' in value) || !Array.isArray(value.pages)
    || !('pageParams' in value) || !Array.isArray(value.pageParams)) return null;
  const pages: unknown[] = value.pages, pageParams: unknown[] = value.pageParams;
  if (pages.length === 0 || pages.length !== pageParams.length || !pages.every(validPage)
    || !pageParams.every((param): param is number | null => param === null || typeof param === 'number' && Number.isSafeInteger(param))) return null;
  return { pages, pageParams };
}
/** A request's start order is not its native snapshot order. Keep all known rows in this
 * admission, bounded to the newest page, and retain actual native tail-exhaustion evidence. */
function mergeKnownHead(consumer: Consumer, page: ThreadRawListReply | null = null): ThreadRawListReply {
  const base = page ?? consumer.head;
  if (base === null) throw retiredPublication();
  if (page !== null && !page.has_older) {
    const last = page.events.at(-1)?.n;
    if (last !== undefined) consumer.exhaustedBelow = consumer.exhaustedBelow === null ? last : Math.min(consumer.exhaustedBelow, last);
  }
  const known = new Map<number, RawEvent>();
  for (const event of consumer.head?.events ?? []) known.set(event.n, event);
  for (const event of page?.events ?? []) known.set(event.n, event);
  for (const [n, event] of consumer.staged) known.set(n, event);
  const ordered = [...known.values()].sort((one, other) => other.n - one.n);
  const events = ordered.slice(0, RAW_PAGE), tail = events.at(-1)?.n;
  return stampThreadReceipt({ ...base, events,
    has_older: ordered.length > RAW_PAGE || tail !== undefined && tail > 1 && (consumer.exhaustedBelow === null || tail > consumer.exhaustedBelow),
  }, consumer.captured);
}

/** A reanchored head may leave a gap before explicitly loaded older pages. Follow coverage,
 * not append order, so that retained older pages never hide that gap or require a bulk refetch. */
function nextBefore(pages: ThreadRawListReply[], params: (number | null)[]): number | undefined {
  const head = pages[0];
  let cursor = head?.has_older === true ? head.events.at(-1)?.n : undefined;
  for (let step = 0; cursor !== undefined && step < pages.length; step += 1) {
    let progressed = false;
    for (let index = 1; index < pages.length; index += 1) {
      const page = pages[index], before = params[index];
      if (page === undefined || before === undefined || before === null || cursor > before) continue;
      if (!page.has_older) return undefined;
      const last = page.events.at(-1)?.n;
      if (last !== undefined && cursor > last) { cursor = last; progressed = true; break; }
    }
    if (!progressed) return cursor;
  }
  return cursor;
}

export function scopedRawFramesQuery(target: ThreadReadTarget, segment: string) {
  const source = new RawDescriptor(target, segment);
  const reading = threadQueryRead(source.ref, ['thread-raw', source.ref.threadId, segment]);
  return infiniteQueryOptions({
    queryKey: reading.key, enabled: reading.enabled && source.captured !== null,
    meta: { scopedRaw: source } satisfies RawQueryMeta,
    initialPageParam: null as number | null,
    queryFn: ({ client: cache, queryKey, signal, pageParam }): Promise<ThreadRawListReply> => {
      const join = installed.get(cache);
      if (join === undefined) return Promise.reject(new RawSourceError('transport_unavailable', 'The live connection is not installed'));
      return join.read(queryKey, source, pageParam, signal);
    },
    getNextPageParam: (_last, pages, _lastParam, params) => nextBefore(pages, params),
    // Merge before observer notification: an older infinite-page completion must not expose
    // its captured old head even briefly after a newer head/live row has been accepted.
    structuralSharing: (previous, next) => {
      if (source.owner === null) throw retiredPublication();
      return source.owner.join.commit(source.owner.query, source, previous, next);
    },
    retry: false, staleTime: Infinity, refetchOnWindowFocus: false,
  });
}

interface Admission { readonly id: number; readonly after: number; readonly attempt: AbortController }
interface Consumer {
  readonly query: Query;
  readonly source: RawDescriptor;
  readonly captured: ThreadReadCapture;
  readonly release: () => void;
  readonly staged: Map<number, RawEvent>;
  readonly controller: AbortController;
  group: Group | null;
  error: Error | null;
  needsHead: boolean;
  reanchoring: boolean;
  head: ThreadRawListReply | null;
  exhaustedBelow: number | null;
  recover: boolean;
}
interface Group {
  readonly key: string;
  readonly feed: string;
  readonly segment: string;
  readonly projection: ThreadContinuationProjection;
  readonly consumers: Set<Consumer>;
  readonly waits: Set<() => void>;
  subscription: Subscription | null;
  admission: Admission | null;
  attempt: AbortController;
  id: number | null;
  closed: boolean;
}
interface Reopening { readonly source: RawDescriptor; readonly release: () => void }
interface PublishedPage { readonly query: Query; readonly captured: ThreadReadCapture; readonly segment: string; readonly authority: object }
interface Publication {
  readonly join: RawJoin;
  readonly query: Query;
  readonly consumer: Consumer;
  readonly admission: Admission;
  readonly signal: AbortSignal;
}

function sourceKey(captured: ThreadReadCapture, segment: string): string {
  const projection = captured.scope.projection;
  if (projection === null) throw threadReadChanged();
  return JSON.stringify([captured.scope.reader, captured.threadId, segment, captured.scope.selection,
    projection.viewed_continuation_id, projection.projection_revision, projection.follows_active,
    projection.follows_active ? projection.execution_revision : null]);
}
function current(consumer: Consumer): boolean {
  return !consumer.controller.signal.aborted && isCurrentThreadRead(consumer.captured);
}

class RawJoin {
  private readonly socket: RealtimeSocket;
  private readonly cache: QueryClient;
  private readonly consumers = new Map<Query, Consumer>();
  private readonly groups = new Map<string, Group>();
  private readonly reopenings = new Map<string, Reopening>();
  private readonly unlisten: (() => void)[] = [];
  private stopStatus: (() => void) | null = null;
  private queued = false;
  private stopped = false;

  constructor(socket: RealtimeSocket, cache: QueryClient) {
    this.socket = socket; this.cache = cache;
    this.unlisten.push(cache.getQueryCache().subscribe((event) => {
      // Resolve the exact cache/owned instance instead of trusting the event's erased generics.
      const cached = cache.getQueryCache().get(event.query.queryHash);
      const query = cached === event.query ? cached : [...this.consumers.keys()].find((held) => held === event.query);
      if (query === undefined) { this.schedule(); return; }
      this.bind(query);
      const consumer = this.consumers.get(query);
      if (consumer !== undefined && event.type === 'removed') this.remove(consumer);
      if (consumer !== undefined && event.type === 'updated') {
        if (event.action.type === 'fetch' && consumer.error !== null) this.remove(consumer);
        else if (event.action.type === 'error') this.fail(consumer, query.state.error instanceof Error ? query.state.error : protocolError());
        else if (event.action.type === 'success') this.flush(consumer);
      }
      if (event.type === 'observerResultsUpdated' || event.type === 'updated' && event.action.type === 'success') return;
      this.schedule();
    }), useThreadStore.subscribe(() => this.schedule()), client.session.subscribe(() => this.schedule()));
    this.reconcile();
  }
  private bind(query: Query): void {
    const source = descriptor(query);
    if (source !== null && (source.owner?.join !== this || source.owner.query !== query)) source.owner = { join: this, query };
  }
  private schedule(): void {
    if (this.queued || this.stopped) return;
    this.queued = true;
    queueMicrotask(() => { this.queued = false; if (!this.stopped) this.reconcile(); });
  }
  private reconcile(): void {
    for (const consumer of this.consumers.values()) {
      if (!consumer.query.isActive() || !current(consumer) || this.cache.getQueryCache().get(consumer.query.queryHash) !== consumer.query) this.remove(consumer);
    }
    const observedOwners = new Set<string>();
    for (const query of this.cache.getQueryCache().getAll()) {
      this.bind(query);
      const source = descriptor(query), captured = source?.captured;
      if (source !== null && query.getObserversCount() > 0 && (query.isActive() || source.captured === null)) observedOwners.add(threadReadCacheKey(source.ref));
      if (source === null || captured === undefined || captured === null || !query.isActive()
        || !isCurrentThreadRead(captured) || this.consumers.has(query)) continue;
      if (query.state.status === 'error' && query.state.fetchStatus !== 'fetching') continue;
      this.add(query, source, captured);
    }
    for (const [key, opening] of this.reopenings) {
      const read = threadViewOf(opening.source.ref), before = opening.source.captured;
      // A visible observer moves to a disabled cold key during the normal owner opening.
      // Keep its owner hold across that key change; a final unsubscribe has no matching owner.
      const observed = observedOwners.has(key);
      if (!observed || before === null || read.read?.identity !== before.scope.identity
        || read.read.reader !== before.scope.reader || read.read.selection !== before.scope.selection
        || read.read.ready || read.error !== null || read.closed !== null) {
        opening.release(); this.reopenings.delete(key);
      }
    }
  }
  private add(query: Query, source: RawDescriptor, captured: ThreadReadCapture): Consumer {
    const held = this.consumers.get(query);
    if (held !== undefined) return held;
    const consumer: Consumer = { query, source, captured, release: holdThreadRead(source.ref), staged: new Map(),
      controller: new AbortController(), group: null, error: null, needsHead: pagesOf(query.state.data) !== null, reanchoring: false,
      head: null, exhaustedBelow: null, recover: pagesOf(query.state.data) === null && query.state.fetchStatus !== 'fetching' };
    this.consumers.set(query, consumer);
    const key = sourceKey(captured, source.segment);
    let group = this.groups.get(key);
    if (group === undefined) {
      const projection = captured.scope.projection;
      if (projection === null) throw threadReadChanged();
      group = { key, feed: `thread:${captured.threadId}:raw`, segment: source.segment, projection,
        consumers: new Set(), waits: new Set(), subscription: null, admission: null, attempt: new AbortController(), id: null, closed: false };
      this.groups.set(key, group);
    }
    consumer.group = group; group.consumers.add(consumer);
    if (group.subscription === null) this.subscribe(group, captured);
    else if (group.admission !== null) this.reanchor(consumer);
    return consumer;
  }
  private subscribe(group: Group, captured: ThreadReadCapture): void {
    this.stopStatus ??= this.socket.status.subscribe((state) => {
      if (state.phase === 'ready') return;
      for (const source of this.groups.values()) this.reset(source);
    });
    const subscription = this.socket.subscribe(group.feed, {
      rawSegmentId: group.segment, projectionRevision: group.projection.projection_revision,
      ...(captured.scope.selection === null ? {} : { continuationId: captured.scope.selection }),
      since: () => null,
      onSubscribe: (id) => { if (!group.closed) { this.reset(group); group.id = id; } },
      onFrame: (frame) => this.receive(group, frame),
    });
    if (group.closed) subscription.unsubscribe(); else group.subscription = subscription;
  }
  private cancelFetch(consumer: Consumer): void {
    void consumer.query.cancel({ revert: false, silent: true });
    if (consumer.query.state.fetchStatus !== 'idle') consumer.query.setState({ fetchStatus: 'idle' });
  }
  private reset(group: Group): void {
    const replacing = group.id !== null || group.admission !== null;
    group.attempt.abort(); group.attempt = new AbortController(); group.admission = null; group.id = null;
    for (const consumer of group.consumers) {
      consumer.staged.clear(); consumer.head = null; consumer.exhaustedBelow = null; consumer.needsHead = true;
      if (replacing) { consumer.recover = true; this.cancelFetch(consumer); }
    }
    this.wake(group);
  }
  private wake(group: Group): void { for (const notify of [...group.waits]) notify(); }
  private detach(consumer: Consumer): void {
    const group = consumer.group;
    consumer.group = null;
    if (group === null) return;
    group.consumers.delete(consumer); this.wake(group);
    if (group.consumers.size !== 0) return;
    group.closed = true; group.attempt.abort(); group.subscription?.unsubscribe(); group.subscription = null;
    this.groups.delete(group.key); this.wake(group);
    if (this.groups.size === 0) { this.stopStatus?.(); this.stopStatus = null; }
  }
  private remove(consumer: Consumer): void {
    if (!this.consumers.delete(consumer.query)) return;
    consumer.controller.abort(); consumer.staged.clear(); this.cancelFetch(consumer); this.detach(consumer); consumer.release();
  }
  private fail(consumer: Consumer, error: Error): void {
    if (consumer.error !== null || !current(consumer)) return;
    consumer.error = stampThreadReceipt(error, consumer.captured); consumer.staged.clear(); consumer.controller.abort(); this.detach(consumer);
    if (isCurrentThreadRead(consumer.captured)) {
      // Retire Query's retryer too: an explicit Retry may start before an ignored HTTP abort settles.
      void consumer.query.cancel({ revert: false, silent: true });
      consumer.query.setState({ data: undefined, error, status: 'error', fetchStatus: 'idle' });
    }
  }
  private restart(group: Group, reload = false): void {
    const captured = [...group.consumers].find(current)?.captured;
    if (captured === undefined) return;
    group.subscription?.unsubscribe(); group.subscription = null;
    this.reset(group);
    if (reload) for (const consumer of group.consumers) {
      // Authority retirement clears every published page, including infinite tails, while
      // metadata keeps the exact local owner and native source available for a fresh ACK.
      publicationAuthorities.set(consumer.query, {});
      consumer.query.setState({ data: undefined, dataUpdatedAt: 0, error: null, errorUpdatedAt: 0,
        status: 'pending', fetchStatus: 'idle' });
    }
    this.subscribe(group, captured);
  }
  private reload(group: Group): void {
    const owners = new Map<string, Consumer[]>();
    for (const consumer of group.consumers) if (current(consumer)) {
      const key = threadReadCacheKey(consumer.source.ref);
      owners.set(key, [...owners.get(key) ?? [], consumer]);
    }
    // Release this old wire source and every pending raw request before any owner opening.
    group.subscription?.unsubscribe(); group.subscription = null;
    for (const [key, consumers] of owners) {
      const first = consumers[0];
      if (first === undefined || !isCurrentThreadRead(first.captured)) continue;
      const opening: Reopening = { source: first.source, release: holdThreadRead(first.source.ref) };
      this.reopenings.get(key)?.release(); this.reopenings.set(key, opening);
      for (const consumer of consumers) this.remove(consumer);
      if (isCurrentThreadRead(first.captured)) threadWriter.invalidate(first.source.ref);
    }
    this.schedule();
  }
  private receive(group: Group, frame: FeedFrame): void {
    if (group.closed || frame.id !== group.id || frame.op !== 'error' && frame.feed !== group.feed) return;
    for (const consumer of [...group.consumers]) if (!current(consumer)) this.remove(consumer);
    if (this.groups.get(group.key) !== group) return;
    if (frame.op === 'subscribed') {
      const projection = readThreadProjection(frame.projection), raw = frame.raw;
      if (projection === null || raw?.segment_id !== group.segment
        || !Number.isSafeInteger(raw.after) || raw.after < 0) {
        for (const consumer of [...group.consumers]) this.fail(consumer, protocolError());
      } else if (frame.mode === 'reload') {
        if (sameThreadProjection(group.projection, projection)) {
          for (const consumer of [...group.consumers]) this.fail(consumer, protocolError());
        } else this.reload(group);
      } else if (frame.mode !== 'ephemeral' || !sameThreadProjection(group.projection, projection)) {
        for (const consumer of [...group.consumers]) this.fail(consumer, protocolError());
      } else {
        group.admission = { id: frame.id, after: raw.after, attempt: group.attempt };
        this.wake(group);
        for (const consumer of group.consumers) this.reanchor(consumer);
      }
      return;
    }
    if (frame.op === 'resync') { this.restart(group, frame.reload === true); return; }
    if (frame.op === 'error' || frame.op === 'closed') {
      if (frame.op === 'closed' && frame.reason === 'unsubscribed') return;
      for (const consumer of [...group.consumers]) this.fail(consumer, new RawSourceError(
        frame.op === 'error' ? frame.code : frame.reason, frame.op === 'error' ? frame.detail : frame.reason));
      return;
    }
    if (frame.op !== 'event' || group.admission === null) return;
    const value: unknown = frame.data;
    if (typeof value !== 'object' || value === null || !('projection' in value) || !('segment_id' in value) || !('event' in value)) return;
    const projection = readThreadProjection(value.projection);
    if (value.segment_id !== group.segment || projection === null || !sameThreadProjection(group.projection, projection)
      || !validEvent(value.event) || value.event.n <= group.admission.after) return;
    for (const consumer of group.consumers) {
      if (!current(consumer) || consumer.error !== null) continue;
      if (!consumer.staged.has(value.event.n) && consumer.staged.size === RAW_PAGE) { this.restart(group); return; }
      consumer.staged.set(value.event.n, value.event);
      this.flush(consumer);
    }
  }
  private returnedPage(consumer: Consumer, page: ThreadRawListReply, admission: Admission, signal: AbortSignal): ThreadRawListReply {
    const returned = stampThreadReceipt({ ...page }, consumer.captured);
    publications.set(returned, { join: this, query: consumer.query, consumer, admission, signal });
    return returned;
  }
  /** This guard remains after consumer removal/manager stop and after queryFn's last await.
   * Only already-published page objects may be retained without a new admission receipt. */
  commit(query: Query, source: RawDescriptor, previous: unknown, value: unknown): unknown {
    const consumer = this.consumers.get(query), next = pagesOf(value), before = pagesOf(previous);
    if (this.stopped || consumer === undefined || next === null || !query.isActive()
      || this.cache.getQueryCache().get(query.queryHash) !== query || !current(consumer) || consumer.error !== null
      || source.segment !== consumer.source.segment || !sameThreadRead(source.ref, consumer.source.ref)
      || source.captured === null || !isCurrentThreadRead(source.captured)) throw retiredPublication();
    const admission = consumer.group?.admission;
    if (admission?.attempt.signal.aborted !== false) throw retiredPublication();
    const authority = publicationAuthority(query);
    let fresh = false;
    for (const page of next.pages) {
      const published = publishedPages.get(page);
      if (published?.query === query && published.segment === source.segment
        && published.authority === authority && isCurrentThreadRead(published.captured)) continue;
      const receipt = publications.get(page);
      if (receipt?.join !== this || receipt.query !== query || receipt.consumer !== consumer
        || receipt.admission !== admission || receipt.signal.aborted) throw retiredPublication();
      fresh = true;
    }
    if (!fresh) {
      // A no-more-pages operation has no new queryFn result. Preserve the latest cache window,
      // not the old infinite snapshot it captured before a concurrent head commit.
      if (before === null) throw retiredPublication();
      return previous;
    }
    // An older page may retain a prior cached head while the fresh admitted head is pending.
    // Do not promote that retained object into this admission's known-head witness.
    const head = consumer.head === null ? null : mergeKnownHead(consumer);
    const result = head === null ? next : { ...next, pages: [head, ...next.pages.slice(1)] };
    if (head !== null) { consumer.head = head; consumer.staged.clear(); consumer.recover = false; }
    for (const page of result.pages) publishedPages.set(page, { query, captured: consumer.captured, segment: source.segment, authority });
    return result;
  }
  private write(consumer: Consumer, head: ThreadRawListReply): void {
    const before = pagesOf(consumer.query.state.data);
    try {
      this.cache.setQueryData<Pages>(consumer.query.queryKey, before === null ? { pages: [head], pageParams: [null] }
        : { ...before, pages: [head, ...before.pages.slice(1)] });
    } catch (error) {
      if (!(error instanceof CancelledError) && current(consumer)) this.fail(consumer, error instanceof Error ? error : protocolError());
    }
  }
  private flush(consumer: Consumer): void {
    if (!current(consumer) || consumer.error !== null || consumer.query.state.fetchStatus !== 'idle' || consumer.reanchoring) return;
    const before = pagesOf(consumer.query.state.data), admission = consumer.group?.admission;
    if (before !== null && consumer.head !== null && admission !== undefined && admission !== null
      && (consumer.staged.size > 0 || consumer.head !== before.pages[0])) {
      consumer.head = mergeKnownHead(consumer);
      this.write(consumer, this.returnedPage(consumer, consumer.head, admission, consumer.controller.signal));
    }
    this.reanchor(consumer);
  }
  private reanchor(consumer: Consumer): void {
    const group = consumer.group;
    if (!current(consumer) || consumer.error !== null || !consumer.needsHead || consumer.reanchoring
      || group?.admission?.attempt.signal.aborted !== false || consumer.query.state.fetchStatus !== 'idle'
      || pagesOf(consumer.query.state.data) === null && !consumer.recover) return;
    consumer.reanchoring = true;
    void this.page(consumer, null, consumer.controller.signal).then((head) => {
      if (current(consumer) && consumer.error === null) this.write(consumer, head);
    }).catch((error: unknown) => {
      if (!(error instanceof CancelledError) && current(consumer)) this.fail(consumer, error instanceof Error ? error : protocolError());
    }).finally(() => {
      consumer.reanchoring = false;
      this.flush(consumer);
    });
  }
  private async admitted(consumer: Consumer, signal: AbortSignal): Promise<Admission> {
    for (;;) {
      if (consumer.error !== null) throw consumer.error;
      assertThreadRead(consumer.captured, signal);
      const group = consumer.group;
      if (group === null || group.closed || consumer.controller.signal.aborted || this.stopped) throw threadReadChanged();
      if (group.admission !== null) return group.admission;
      await new Promise<void>((resolve, reject) => {
        const abort = () => { cleanup(); reject(consumer.error ?? threadReadChanged()); };
        const inspect = () => { cleanup(); resolve(); };
        const cleanup = () => {
          group.waits.delete(inspect); signal.removeEventListener('abort', abort);
          consumer.controller.signal.removeEventListener('abort', abort);
          consumer.captured.scope.controller.signal.removeEventListener('abort', abort);
        };
        group.waits.add(inspect); signal.addEventListener('abort', abort, { once: true });
        consumer.controller.signal.addEventListener('abort', abort, { once: true });
        consumer.captured.scope.controller.signal.addEventListener('abort', abort, { once: true });
        if (signal.aborted || consumer.controller.signal.aborted || consumer.captured.scope.controller.signal.aborted) abort();
        else if (group.admission !== null || group.closed || consumer.error !== null) inspect();
      });
    }
  }
  private async page(consumer: Consumer, before: number | null, signal: AbortSignal): Promise<ThreadRawListReply> {
    for (;;) {
      const admission = await this.admitted(consumer, signal);
      const requestSignal = AbortSignal.any([signal, consumer.controller.signal, admission.attempt.signal]);
      try {
        const page = await interrupt(readInThread(consumer.captured, (readSignal) => ok(api.GET('/threads/{id}/raw', {
          params: { path: { id: consumer.captured.threadId }, query: { segment_id: consumer.source.segment, limit: RAW_PAGE,
            ...threadReadSelector(consumer.captured), ...(before === null ? {} : { before }) } }, signal: readSignal,
        })), requestSignal), requestSignal);
        if (!validPage(page)) throw protocolError();
        receiveThreadRead(consumer.captured, page.projection, requestSignal);
        if (consumer.group?.admission !== admission || admission.attempt.signal.aborted) continue;
        if (before !== null) return this.returnedPage(consumer, page, admission, requestSignal);
        consumer.needsHead = false;
        consumer.head = mergeKnownHead(consumer, page);
        return this.returnedPage(consumer, consumer.head, admission, requestSignal);
      } catch (error) {
        if (consumer.error !== null) throw consumer.error;
        if (signal.aborted || !current(consumer)) throw threadReadChanged();
        if (admission.attempt.signal.aborted) continue;
        throw error;
      }
    }
  }
  read(key: QueryKey, source: RawDescriptor, before: number | null, signal: AbortSignal): Promise<ThreadRawListReply> {
    const captured = source.captured;
    if (captured === null || !isCurrentThreadRead(captured, signal)) return Promise.reject(threadReadChanged());
    const query = this.cache.getQueryCache().find({ queryKey: key, exact: true });
    if (query?.isActive() !== true) return Promise.reject(threadReadChanged());
    this.bind(query);
    const existing = this.consumers.get(query);
    if (existing !== undefined && existing.error !== null) this.remove(existing);
    const consumer = this.consumers.get(query) ?? this.add(query, source, captured);
    return this.page(consumer, before, signal);
  }
  stop(): void {
    if (this.stopped) return;
    this.stopped = true;
    for (const stop of this.unlisten) stop();
    for (const consumer of [...this.consumers.values()]) this.remove(consumer);
    for (const opening of this.reopenings.values()) opening.release();
    this.reopenings.clear(); this.stopStatus?.(); this.stopStatus = null;
  }
}

/** One manager per existing application query cache, released with its socket integration. */
export function installRawQueryFeeds(socket: RealtimeSocket, cache: QueryClient): () => void {
  const previous = installed.get(cache);
  previous?.stop();
  const join = new RawJoin(socket, cache);
  installed.set(cache, join);
  return () => { if (installed.get(cache) === join) installed.delete(cache); join.stop(); };
}
