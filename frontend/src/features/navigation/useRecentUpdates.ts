/** Visible recent pages follow the existing personal-visit and reconnect feed, without another subscription. */
import { hashKey, type QueryKey, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { onUserEvent } from '@/realtime/stores/user';
import { listingLostAccess } from './recent';

/** A later network failure cannot revive data already refused by the server. */
export function useListingRetired(key: QueryKey, error: unknown): boolean {
  const cache = useQueryClient();
  const query = cache.getQueryCache().get(hashKey(key));
  const generation = query?.state.dataUpdateCount ?? 0;
  const [cut, setCut] = useState<{ query: object; generation: number } | null>(null);
  const refused = listingLostAccess(error);
  if (refused && query !== undefined && (cut?.query !== query || cut.generation !== generation)) setCut({ query, generation });
  if (!refused && query?.state.status === 'success' && cut !== null) setCut(null);
  return refused || query === undefined && cut !== null || query?.state.status !== 'success' && cut?.query === query && cut?.generation === generation;
}

export function useRecentUpdates(key: QueryKey, enabled: boolean, visits: boolean): void {
  const cache = useQueryClient();
  const hash = hashKey(key);
  useEffect(() => {
    if (!enabled) return;
    const refresh = () => {
      const query = cache.getQueryCache().get(hash);
      if (query?.isActive()) void cache.invalidateQueries({ queryKey: query.queryKey, exact: true });
    };
    const stopVisits = visits ? onUserEvent('recents', refresh) : () => undefined;
    const stopConnected = onUserEvent('connected', (data) => { if (data.resumed === true) refresh(); });
    return () => { stopVisits(); stopConnected(); };
  }, [cache, hash, enabled, visits]);
}
