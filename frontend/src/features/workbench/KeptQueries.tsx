/** An open view owns its cached answers independently of its visible readers. */
import { QueryClient, QueryClientContext, QueryClientProvider, QueryObserver, type Query } from '@tanstack/react-query';
import { createContext, type ReactNode, useContext, useLayoutEffect, useMemo, useState } from 'react';
import { isBootstrapRead, READ_NOW } from '@/api/freshness';
import { shellBootstrapIndex } from '@/api/bootstrapIndex';
import { startingRead } from '@/api/readScheduler';
import { cancelRead, hasReadConsumer, isReadRetainer, readVisibility, retainRead } from '@/api/reads';

const viewReads = new WeakMap<QueryClient, { keys: ReadonlySet<string>; options: WeakSet<object> }>();
const scopeVisibility = new WeakMap<QueryClient, () => boolean>();
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
  readonly hiddenClient: QueryClient;
  readonly paintClient: QueryClient;
  private mode: 'shown' | 'hidden' | 'paint' = 'shown';
  private keys = new Set<string>();
  private options = new WeakSet<object>();
  private held = new Map<string, HeldRead>();
  private observed = new Set<string>();
  private pruning = false;
  private unsubscribe?: () => void;
  private owner: QueryClient;
  private parentShown: () => boolean = () => true;
  private shown = () => this.mode !== 'hidden' && this.parentShown();

  constructor(owner: QueryClient) {
    this.owner = owner;
    this.parentShown = scopeVisibility.get(owner) ?? (() => true);
    const cache = owner.getQueryCache();
    const bootstraps = shellBootstrapIndex(cache);
    const makeClient = (mode: 'shown' | 'hidden' | 'paint') => {
      const client = new QueryClient({ queryCache: owner.getQueryCache(), mutationCache: owner.getMutationCache(), defaultOptions: owner.getDefaultOptions() });
      const resolved = new WeakSet<object>();
      const nativeDefaults = client.defaultQueryOptions.bind(client);
      const remember: QueryClient['defaultQueryOptions'] = (options) => {
        // Re-delivering an answered reader's options does not change its policy. Missing,
        // failed or opening-supplied answers must still be judged when the observer attaches.
        if (resolved.has(options) && options.queryHash !== undefined) {
          const current = cache.get(options.queryHash)?.state;
          if (current?.status === 'success' && current.data !== undefined && startingRead(options.queryKey).staleTime === undefined) return nativeDefaults(options);
        }
        const defaults = owner.defaultQueryOptions(options);
        this.keys.add(defaults.queryHash);
        // A failed part gets its own native read again on return, even when an aggregate supplied it.
        const state = cache.get(defaults.queryHash)?.state;
        const bootstrap = bootstraps.first;
        const suppliedSoon = state?.data === undefined && bootstrap !== undefined && bootstrap.state.data === undefined && isBootstrapRead(defaults.queryKey);
        const paused = mode === 'hidden' || mode === 'paint' && state?.data !== undefined || suppliedSoon;
        const retry = defaults.retry;
        const guarded = typeof retry !== 'function' ? defaults : { ...defaults, retry: (...args: Parameters<typeof retry>) => {
          const query = owner.getQueryCache().get(defaults.queryHash);
          return (this.shown() || query !== undefined && hasReadConsumer(query)) && retry(...args);
        } };
        const result = paused ? { ...guarded, enabled: false as const, refetchInterval: false as const }
          : state?.status === 'error'
            ? { ...guarded, ...READ_NOW, retryOnMount: true, refetchOnMount: 'always' as const } : guarded;
        this.options.add(result);
        if (state?.status === 'success' && state.data !== undefined) resolved.add(result);
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
      viewReads.set(client, { keys: this.keys, options: this.options });
      scopeVisibility.set(client, this.shown);
      return client;
    };
    // All data, mutations and key defaults still belong to the application's one cache.
    this.client = makeClient('shown');
    this.hiddenClient = makeClient('hidden');
    this.paintClient = makeClient('paint');
  }

  setMode(mode: 'shown' | 'hidden' | 'paint'): void {
    const previous = this.mode;
    this.mode = mode;
    if (mode === 'shown') { if (previous !== mode) this.pruneLater(); return; }
    for (const key of this.keys) {
      const query = this.owner.getQueryCache().get(key);
      if (query === undefined || mode === 'paint' && query.state.data === undefined) continue;
      for (const observer of query.observers) if (this.options.has(observer.options) && observer.options.enabled !== false) {
        const options = { ...observer.options, enabled: false as const, refetchInterval: false as const };
        this.options.add(options);
        readVisibility(options, this.shown);
        observer.setOptions(options);
      }
    }
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
      const own = (event.type === 'observerAdded' || event.type === 'observerOptionsUpdated')
        && !isReadRetainer(event.observer) && this.options.has(event.observer.options);
      if (own) this.keys.add(event.query.queryHash);
      if (event.type === 'observerAdded' && own) this.observed.add(event.query.queryHash);
      if (!this.keys.has(event.query.queryHash)) return;
      const query = this.owner.getQueryCache().get(event.query.queryHash);
      if (event.type === 'removed' && query !== undefined && query !== event.query) return;
      if (event.type === 'observerAdded' && this.options.has(event.observer.options)
        && (this.mode === 'hidden' || this.mode === 'paint' && event.query.state.data !== undefined)
        && event.observer.options.enabled !== false) {
        // Activity reconnects subscriptions before useQuery applies its new options effect.
        // Pause that observer now so its previous visible options cannot fetch before paint.
        const options = { ...event.observer.options, enabled: false as const, refetchInterval: false as const };
        this.options.add(options);
        readVisibility(options, this.shown);
        event.observer.setOptions(options);
      }
      if (event.type === 'removed') {
        const held = this.held.get(event.query.queryHash);
        this.held.delete(event.query.queryHash);
        held?.release();
      } else if (event.type === 'observerAdded' || event.type === 'observerRemoved' || event.type === 'observerOptionsUpdated') {
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
  const parentReady = useContext(ViewReadsReady);
  const owner = useContext(QueryClientContext);
  const kept = useMemo(() => owner === undefined ? null : new KeptQueries(owner), [owner]);
  const [reads, setReads] = useState({ shown, ready: shown });
  if (reads.shown !== shown) setReads({ shown, ready: false });
  useLayoutEffect(() => {
    if (!shown || reads.ready) return;
    let afterPaint = 0;
    // A frame callback precedes paint. Its successor lets kept content paint before readers reconnect.
    const frame = requestAnimationFrame(() => { afterPaint = requestAnimationFrame(() => setReads({ shown: true, ready: true })); });
    return () => { cancelAnimationFrame(frame); cancelAnimationFrame(afterPaint); };
  }, [shown, reads.ready]);
  useLayoutEffect(() => kept?.start(), [kept]);
  const ready = parentReady && shown && reads.shown && reads.ready;
  const mode = !shown ? 'hidden' : ready ? 'shown' : 'paint';
  useLayoutEffect(() => { kept?.setMode(mode); }, [kept, mode]);
  return <ViewReadsReady.Provider value={ready}>{kept === null ? children : <QueryClientProvider client={mode === 'shown' ? kept.client : mode === 'paint' ? kept.paintClient : kept.hiddenClient}>{children}</QueryClientProvider>}</ViewReadsReady.Provider>;
}
