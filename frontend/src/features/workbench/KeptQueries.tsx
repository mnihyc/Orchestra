/** An open view owns its cached answers independently of its visible readers. */
import { QueryClient, QueryClientContext, QueryClientProvider, QueryObserver, type Query } from '@tanstack/react-query';
import { createContext, type ReactNode, useContext, useLayoutEffect, useMemo, useState } from 'react';
import { isBootstrapRead, READ_NOW } from '@/api/freshness';
import { client as sessionClient } from '@/api/client';
import { cancelRead, hasReadConsumer, readVisibility, retainRead } from '@/api/reads';

const viewReads = new WeakMap<QueryClient, WeakSet<object>>();
const scopeVisibility = new WeakMap<QueryClient, () => boolean>();
const ViewReadsReady = createContext(true);
/** Raw feed readers reconnect after a kept view has painted; ordinary query readers use the scope automatically. */
export function useViewReadsReady(): boolean { return useContext(ViewReadsReady); }

/** The initial reads that this view actually owns, independent of unrelated panes or shell readers. */
export function viewReadState(cache: QueryClient): { pending: boolean; failed: boolean } {
  const options = viewReads.get(cache);
  const reads = cache.getQueryCache().getAll().filter((query) => query.state.data === undefined && query.observers.some((observer) => {
    if (options !== undefined && !options.has(observer.options)) return false;
    const enabled = observer.options.enabled;
    return typeof enabled === 'function' ? enabled(query) : enabled !== false;
  }));
  return { pending: reads.some((query) => query.state.status === 'pending'), failed: reads.some((query) => query.state.status === 'error') };
}

class KeptQueries {
  readonly client: QueryClient;
  readonly hiddenClient: QueryClient;
  readonly paintClient: QueryClient;
  private mode: 'shown' | 'hidden' | 'paint' = 'shown';
  private keys = new Set<string>();
  private options = new WeakSet<object>();
  private held = new Map<string, () => void>();
  private unsubscribe?: () => void;
  private owner: QueryClient;
  private parentShown: () => boolean = () => true;
  private shown = () => this.mode !== 'hidden' && this.parentShown();

  constructor(owner: QueryClient) {
    this.owner = owner;
    this.parentShown = scopeVisibility.get(owner) ?? (() => true);
    const makeClient = (mode: 'shown' | 'hidden' | 'paint') => {
      const client = new QueryClient({ queryCache: owner.getQueryCache(), mutationCache: owner.getMutationCache(), defaultOptions: owner.getDefaultOptions() });
      const remember: QueryClient['defaultQueryOptions'] = (options) => {
        const defaults = owner.defaultQueryOptions(options);
        this.keys.add(defaults.queryHash);
        // A failed part gets its own native read again on return, even when an aggregate supplied it.
        const bootstrap = owner.getQueryState(['shell-bootstrap', sessionClient.session.getState().userId ?? '']);
        const suppliedSoon = isBootstrapRead(defaults.queryKey) && bootstrap !== undefined && bootstrap.data === undefined && owner.getQueryData(defaults.queryKey) === undefined;
        const paused = mode === 'hidden' || mode === 'paint' && owner.getQueryData(defaults.queryKey) !== undefined || suppliedSoon;
        const result = paused ? { ...defaults, enabled: false as const, refetchInterval: false as const }
          : owner.getQueryState(defaults.queryKey)?.status === 'error'
            ? { ...defaults, ...READ_NOW, retryOnMount: true, refetchOnMount: 'always' as const } : defaults;
        this.options.add(result);
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
      viewReads.set(client, this.options);
      scopeVisibility.set(client, this.shown);
      return client;
    };
    // All data, mutations and key defaults still belong to the application's one cache.
    this.client = makeClient('shown');
    this.hiddenClient = makeClient('hidden');
    this.paintClient = makeClient('paint');
  }

  setMode(mode: 'shown' | 'hidden' | 'paint'): void {
    this.mode = mode;
    if (mode === 'shown') return;
    for (const key of this.keys) {
      const query = this.owner.getQueryCache().get(key);
      if (query === undefined || mode === 'paint' && query.state.data === undefined) continue;
      for (const observer of query.observers) if (this.options.has(observer.options) && observer.options.enabled !== false) {
        observer.setOptions({ ...observer.options, enabled: false, refetchInterval: false });
      }
    }
  }

  private keep(query: Query): void {
    if (this.held.has(query.queryHash)) return;
    this.held.set(query.queryHash, () => undefined);
    // A disabled observer prevents collection without refetching, polling or joining a live feed.
    const observer = new QueryObserver(this.owner, { ...query.options, queryKey: query.queryKey, enabled: false, refetchInterval: false });
    retainRead(observer);
    this.held.set(query.queryHash, observer.subscribe(() => undefined));
  }

  start(): () => void {
    this.unsubscribe = this.owner.getQueryCache().subscribe((event) => {
      if (!this.keys.has(event.query.queryHash)) return;
      const query = this.owner.getQueryCache().get(event.query.queryHash);
      if (event.type === 'removed' && query !== undefined && query !== event.query) return;
      if (event.type === 'observerAdded' && this.options.has(event.observer.options)
        && (this.mode === 'hidden' || this.mode === 'paint' && event.query.state.data !== undefined)
        && event.observer.options.enabled !== false) {
        // Activity reconnects subscriptions before useQuery applies its new options effect.
        // Pause that observer now so its previous visible options cannot fetch before paint.
        event.observer.setOptions({ ...event.observer.options, enabled: false, refetchInterval: false });
      }
      if (event.type === 'removed') {
        const release = this.held.get(event.query.queryHash);
        this.held.delete(event.query.queryHash);
        release?.();
      } else if (event.type === 'observerAdded' || event.type === 'observerRemoved' || event.type === 'observerOptionsUpdated') {
        if (query !== undefined) this.keep(query);
        if (query !== undefined && !hasReadConsumer(query) && query.state.fetchStatus === 'fetching') {
          // Retaining an answer is no demand for an unfinished read after its last reader leaves.
          void cancelRead(query);
        }
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
      for (const [key, release] of held) {
        const query = this.owner.getQueryCache().get(key);
        if (query !== undefined && !hasReadConsumer(query)) void cancelRead(query);
        release();
      }
    };
  }
}

export function KeptQueryScope({ children, shown }: { children: ReactNode; shown: boolean }) {
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
  const ready = shown && reads.shown && reads.ready;
  const mode = !shown ? 'hidden' : ready ? 'shown' : 'paint';
  useLayoutEffect(() => { kept?.setMode(mode); }, [kept, mode]);
  return <ViewReadsReady.Provider value={ready}>{kept === null ? children : <QueryClientProvider client={mode === 'shown' ? kept.client : mode === 'paint' ? kept.paintClient : kept.hiddenClient}>{children}</QueryClientProvider>}</ViewReadsReady.Provider>;
}
