/** An open view owns its cached answers independently of its visible readers. */
import { QueryClient, QueryClientContext, QueryClientProvider, QueryObserver, type Query } from '@tanstack/react-query';
import { createContext, type ReactNode, useContext, useInsertionEffect, useLayoutEffect, useState } from 'react';
import { isBootstrapRead, READ_NOW } from '@/api/freshness';
import { shellBootstrapIndex } from '@/api/bootstrapIndex';
import { startingRead } from '@/api/readScheduler';
import { cancelRead, hasReadConsumer, isReadRetainer, readVisibility, retainRead } from '@/api/reads';

type Mode = 'shown' | 'hidden' | 'paint';
interface ReadReceipt { mode: Mode; answered: boolean; policy: () => object }
const viewReads = new WeakMap<QueryClient, { owner: QueryClient; keys: ReadonlySet<string>; options: WeakMap<object, ReadReceipt> }>();
const ParentReadMode = createContext<Mode>('shown');
const ViewReadsReady = createContext(true);
interface HeldRead { query: Query; release: () => void }
const sharedHolds = new WeakMap<Query, { users: number; stop: () => void }>();

/** One disabled observer keeps an answer alive for every tab that owns it. */
function holdAnswer(client: QueryClient, query: Query): () => void {
  const existing = sharedHolds.get(query);
  const hold = existing ?? { users: 0, stop: () => undefined };
  hold.users++;
  if (existing === undefined) {
    // Subscribing can synchronously notify a second scope that needs the same query.
    sharedHolds.set(query, hold);
    const observer = new QueryObserver(client, { ...query.options, queryKey: query.queryKey, enabled: false, staleTime: Infinity, refetchInterval: false });
    retainRead(observer);
    hold.stop = observer.subscribe(() => undefined);
  }
  return () => {
    if (--hold.users !== 0) return;
    sharedHolds.delete(query);
    hold.stop();
  };
}
/** Raw feed readers reconnect after a kept view has painted; ordinary query readers use the scope automatically. */
export function useViewReadsReady(): boolean { return useContext(ViewReadsReady); }

/** The initial reads that this view actually owns, independent of unrelated panes or shell readers. */
export function viewReadState(cache: QueryClient): { pending: boolean; failed: boolean } {
  const scope = viewReads.get(cache);
  const queries = cache.getQueryCache();
  const candidates = scope === undefined ? queries.getAll() : Array.from(scope.keys, (key) => queries.get(key));
  let pending = false, failed = false;
  for (const query of candidates) {
    if (query === undefined || query.state.data !== undefined) continue;
    const shown = query.observers.some((observer) => {
      if (scope !== undefined && !scope.options.has(observer.options)) return false;
      const enabled = observer.options.enabled;
      return typeof enabled === 'function' ? enabled(query) : enabled !== false;
    });
    if (shown) {
      pending ||= query.state.status === 'pending';
      failed ||= query.state.status === 'error';
    }
  }
  return { pending, failed };
}

class KeptQueries {
  readonly client: QueryClient;
  private keys = new Set<string>();
  private options = new WeakMap<object, ReadReceipt>();
  private held = new Map<string, HeldRead>();
  private observed = new Set<string>();
  private pruning = false;
  private unsubscribe?: () => void;
  private owner: QueryClient;
  private mode: Mode;
  private shown = () => this.mode !== 'hidden';

  constructor(parent: QueryClient, mode: Mode) {
    // An enclosing view contributes its mode. Native defaults always come from the cache owner.
    const owner = viewReads.get(parent)?.owner ?? parent;
    this.owner = owner;
    this.mode = mode;
    const cache = owner.getQueryCache();
    const bootstraps = shellBootstrapIndex(cache);
    const client = new QueryClient({ queryCache: owner.getQueryCache(), mutationCache: owner.getMutationCache(), defaultOptions: owner.getDefaultOptions() });
    this.client = client;
    const nativeDefaults = client.defaultQueryOptions.bind(client);
    const remember: QueryClient['defaultQueryOptions'] = (options) => {
      // Re-delivering an answered reader's options does not change its policy. Missing,
      // failed or opening-supplied answers must still be judged when the observer attaches.
      const prior = this.options.get(options);
      if (prior?.mode === this.mode && prior.answered && options.queryHash !== undefined) {
        const current = cache.get(options.queryHash)?.state;
        if (current?.status === 'success' && current.data !== undefined && startingRead(options.queryKey).staleTime === undefined) return nativeDefaults(options);
      }
      // Resolve the caller's current native policy again on return. The opening that once
      // supplied this answer may have retired. The adapter keeps its other observer fields.
      const defaults = prior === undefined ? owner.defaultQueryOptions(options) : nativeDefaults({ ...options, ...prior.policy() });
      this.keys.add(defaults.queryHash);
      // A failed part gets its own native read again on return, even when an aggregate supplied it.
      const state = cache.get(defaults.queryHash)?.state;
      const bootstrap = bootstraps.first;
      const suppliedSoon = state?.data === undefined && bootstrap !== undefined && bootstrap.state.data === undefined && isBootstrapRead(defaults.queryKey);
      const paused = this.mode === 'hidden' || this.mode === 'paint' && state?.data !== undefined || suppliedSoon;
      const retry = defaults.retry;
      const guarded = typeof retry !== 'function' ? defaults : { ...defaults, retry: (...args: Parameters<typeof retry>) => {
        const query = owner.getQueryCache().get(defaults.queryHash);
        return (this.shown() || query !== undefined && hasReadConsumer(query)) && retry(...args);
      } };
      const result = paused ? { ...guarded, enabled: false as const, refetchInterval: false as const }
        : state?.status === 'error'
          ? { ...guarded, ...READ_NOW, retryOnMount: true, refetchOnMount: 'always' as const } : guarded;
      const policy = prior?.policy ?? (() => {
        const { enabled, refetchInterval, retry, staleTime, retryOnMount, refetchOnMount } = owner.defaultQueryOptions(options);
        return { enabled, refetchInterval, retry, staleTime, retryOnMount, refetchOnMount };
      });
      const answered = state?.status === 'success' && state.data !== undefined
        && !(defaults.refetchOnMount === false && defaults.retryOnMount === false && defaults.staleTime === Infinity && startingRead(defaults.queryKey).staleTime !== undefined);
      this.options.set(result, { mode: this.mode, answered, policy });
      readVisibility(result, this.shown);
      return result;
    };
    client.defaultQueryOptions = remember;
    client.defaultMutationOptions = owner.defaultMutationOptions.bind(owner);
    client.invalidateQueries = owner.invalidateQueries.bind(owner);
    client.query = owner.query.bind(owner);
    client.setQueryDefaults = owner.setQueryDefaults.bind(owner);
    client.getQueryDefaults = owner.getQueryDefaults.bind(owner);
    client.setMutationDefaults = owner.setMutationDefaults.bind(owner);
    client.getMutationDefaults = owner.getMutationDefaults.bind(owner);
    client.setDefaultOptions = owner.setDefaultOptions.bind(owner);
    client.getDefaultOptions = owner.getDefaultOptions.bind(owner);
    viewReads.set(client, { owner, keys: this.keys, options: this.options });
  }

  commitMode(mode: Mode): void { this.mode = mode; }

  private applyMode(observer: Query['observers'][number]): void {
    const receipt = this.options.get(observer.options);
    if (receipt !== undefined && receipt.mode !== this.mode && !isReadRetainer(observer)) observer.setOptions(observer.options);
  }

  updateObservers(): void {
    for (const key of this.keys) {
      const query = this.owner.getQueryCache().get(key);
      if (query === undefined) continue;
      // A reader can unsubscribe in its notification. Its neighbour must still receive the mode.
      for (const observer of query.observers.slice()) if (observer.hasListeners()) this.applyMode(observer);
    }
    if (this.mode === 'shown') this.pruneLater();
  }

  private keep(query: Query): void {
    const before = this.held.get(query.queryHash);
    if (before?.query === query) return;
    const held: HeldRead = { query, release: () => undefined };
    // Publish the new owner before releasing the old one, whose cache events can reenter this method.
    this.held.set(query.queryHash, held);
    before?.release();
    const release = holdAnswer(this.owner, query);
    if (this.held.get(query.queryHash) === held) held.release = release;
    else release();
  }

  private pruneLater(): void {
    if (this.pruning) return;
    this.pruning = true;
    queueMicrotask(() => {
      this.pruning = false;
      if (this.unsubscribe === undefined || this.mode !== 'shown') return;
      for (const key of this.observed) {
        const query = this.owner.getQueryCache().get(key);
        if (query?.observers.some((observer) => !isReadRetainer(observer) && this.options.has(observer.options)) === true) continue;
        this.observed.delete(key); this.keys.delete(key);
        const held = this.held.get(key); this.held.delete(key); held?.release();
      }
    });
  }

  start(): () => void {
    this.unsubscribe = this.owner.getQueryCache().subscribe((event) => {
      if (event.type !== 'observerAdded' && event.type !== 'observerOptionsUpdated' && event.type !== 'observerRemoved' && event.type !== 'removed') return;
      const own = (event.type === 'observerAdded' || event.type === 'observerOptionsUpdated')
        && !isReadRetainer(event.observer) && this.options.has(event.observer.options);
      if (own) this.keys.add(event.query.queryHash);
      if (event.type === 'observerAdded' && own) this.observed.add(event.query.queryHash);
      if (!this.keys.has(event.query.queryHash)) return;
      const query = this.owner.getQueryCache().get(event.query.queryHash);
      if (event.type === 'removed' && query !== undefined && query !== event.query) return;
      // Activity can reconnect an observer before its options effect. Correct its mode before it fetches.
      if (own && event.type === 'observerAdded') this.applyMode(event.observer);
      if (event.type === 'removed') {
        const held = this.held.get(event.query.queryHash);
        this.held.delete(event.query.queryHash);
        held?.release();
      } else {
        if (query !== undefined) this.keep(query);
        if (query?.state.data !== undefined && !hasReadConsumer(query) && query.state.fetchStatus === 'fetching') {
          // A cached refresh can stop. An admitted first answer finishes into its retained model,
          // so returning does not replace the same request; hidden retry demand stays paused.
          void cancelRead(query);
        }
        if (event.type === 'observerRemoved') this.pruneLater();
      }
    });
    for (const key of this.keys) {
      const query = this.owner.getQueryCache().get(key);
      if (query !== undefined) this.keep(query);
    }
    return () => {
      this.unsubscribe?.();
      this.unsubscribe = undefined;
      const held = [...this.held.entries()];
      this.held.clear();
      for (const [key, entry] of held) {
        const query = this.owner.getQueryCache().get(key);
        if (query === entry.query && !hasReadConsumer(query)) void cancelRead(query);
        entry.release();
      }
    };
  }
}

export function KeptQueryScope({ children, shown }: { children: ReactNode; shown: boolean }) {
  const parentMode = useContext(ParentReadMode);
  const owner = useContext(QueryClientContext);
  const [reads, setReads] = useState({ shown, ready: shown });
  if (reads.shown !== shown) setReads({ shown, ready: false });
  useLayoutEffect(() => {
    if (!shown || reads.ready) return;
    let afterPaint = 0;
    // A frame callback precedes paint. Its successor lets kept content paint before readers reconnect.
    const frame = requestAnimationFrame(() => { afterPaint = requestAnimationFrame(() => setReads({ shown: true, ready: true })); });
    return () => { cancelAnimationFrame(frame); cancelAnimationFrame(afterPaint); };
  }, [shown, reads.ready]);
  const mode = !shown || parentMode === 'hidden' ? 'hidden' : parentMode === 'shown' && reads.shown && reads.ready ? 'shown' : 'paint';
  const makeScope = () => ({ owner, kept: owner === undefined ? null : new KeptQueries(owner, mode) });
  const [scope, setScope] = useState(makeScope);
  if (scope.owner !== owner) setScope(makeScope());
  const kept = scope.kept;
  // Publish intent only on commit, before descendant layout effects can attach readers.
  useInsertionEffect(() => { kept?.commitMode(mode); }, [kept, mode]);
  useLayoutEffect(() => kept?.start(), [kept]);
  useLayoutEffect(() => { kept?.updateObservers(); }, [kept, mode]);
  return <ParentReadMode.Provider value={mode}><ViewReadsReady.Provider value={mode === 'shown'}>
    {kept === null ? children : <QueryClientProvider client={kept.client}>{children}</QueryClientProvider>}
  </ViewReadsReady.Provider></ParentReadMode.Provider>;
}
