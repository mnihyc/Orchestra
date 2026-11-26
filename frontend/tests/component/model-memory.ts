/** A served personal baseline for picker/start cases that replace the real API with local functions. */
import { QueryClient } from '@tanstack/react-query';
import { client } from '@/api/client';
import { modelHistoryKey } from '@/api/modelHistory';
import { meStateQuery } from '@/api/personal';
import type { LastSetup } from '@/features/start/lastSetup';

export function modelMemoryClient(setup: LastSetup | null = null, cache = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })): QueryClient {
  cache.setQueryData(meStateQuery.queryKey, setup === null ? {} : { thread_setup: setup });
  const key = modelHistoryKey(client.session.getState().userId);
  cache.setQueryDefaults(key, { staleTime: Infinity });
  cache.setQueryData(key, { recent: [], recent_count: 0 });
  return cache;
}
