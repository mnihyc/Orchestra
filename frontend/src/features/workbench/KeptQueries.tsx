/** An open view owns its cached answers independently of its visible readers. */
import { QueryClient, QueryClientContext, QueryClientProvider, QueryObserver, type Query } from '@tanstack/react-query';
import { type ReactNode, useContext, useLayoutEffect, useMemo } from 'react';

class KeptQueries {
  readonly client: QueryClient;
  readonly hiddenClient: QueryClient;
  private keys = new Set<string>();
  private held = new Map<string, () => void>();
  private unsubscribe?: () => void;
  private owner: QueryClient;

  constructor(owner: QueryClient) {
    this.owner = owner;
    const makeClient = (shown: boolean) => {
      const client = new QueryClient({ queryCache: owner.getQueryCache(), mutationCache: owner.getMutationCache(), defaultOptions: owner.getDefaultOptions() });
      const remember: QueryClient['defaultQueryOptions'] = (options) => {
        const defaults = owner.defaultQueryOptions(options);
        this.keys.add(defaults.queryHash);
        if (!shown) return { ...defaults, enabled: false, refetchInterval: false };
        // A failed part gets its own native read again on return, even when an aggregate supplied it.
        return owner.getQueryState(defaults.queryKey)?.status === 'error'
          ? { ...defaults, retryOnMount: true, refetchOnMount: 'always' } : defaults;
      };
      client.defaultQueryOptions = remember;
      client.defaultMutationOptions = owner.defaultMutationOptions.bind(owner);
      client.invalidateQueries = owner.invalidateQueries.bind(owner);
      return client;
    };
    // All data, mutations and key defaults still belong to the application's one cache.
    this.client = makeClient(true);
    this.hiddenClient = makeClient(false);
  }

  private keep(query: Query): void {
    if (this.held.has(query.queryHash)) return;
    this.held.set(query.queryHash, () => undefined);
    // A disabled observer prevents collection without refetching, polling or joining a live feed.
    const observer = new QueryObserver(this.owner, { ...query.options, queryKey: query.queryKey, enabled: false, refetchInterval: false });
    this.held.set(query.queryHash, observer.subscribe(() => undefined));
  }

  start(): () => void {
    this.unsubscribe = this.owner.getQueryCache().subscribe((event) => {
      if (!this.keys.has(event.query.queryHash)) return;
      if (event.type === 'removed') {
        const release = this.held.get(event.query.queryHash);
        this.held.delete(event.query.queryHash);
        release?.();
      } else if ((event.type === 'observerAdded' || event.type === 'observerOptionsUpdated') && event.query.isActive()) {
        const release = this.held.get(event.query.queryHash);
        this.held.delete(event.query.queryHash);
        release?.();
      } else if ((event.type === 'observerRemoved' || event.type === 'observerOptionsUpdated') && !event.query.isActive()) {
        const query = this.owner.getQueryCache().get(event.query.queryHash);
        if (query !== undefined) this.keep(query);
        if (event.query.state.fetchStatus === 'fetching') {
          // Retaining an answer is no demand for an unfinished read after its last reader leaves.
          void event.query.cancel({ revert: true });
        }
      }
    });
    for (const key of this.keys) {
      const query = this.owner.getQueryCache().get(key);
      if (query !== undefined && !query.isActive()) this.keep(query);
    }
    return () => {
      this.unsubscribe?.();
      this.unsubscribe = undefined;
      const held = [...this.held.values()];
      this.held.clear();
      for (const release of held) release();
    };
  }
}

export function KeptQueryScope({ children, shown }: { children: ReactNode; shown: boolean }) {
  const owner = useContext(QueryClientContext);
  const kept = useMemo(() => owner === undefined ? null : new KeptQueries(owner), [owner]);
  useLayoutEffect(() => kept?.start(), [kept]);
  return kept === null ? children : <QueryClientProvider client={shown ? kept.client : kept.hiddenClient}>{children}</QueryClientProvider>;
}
