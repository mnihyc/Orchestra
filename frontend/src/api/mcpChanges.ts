/** Each visible environment or thread connection list keeps its own admitted query. */
import type { QueryClient } from '@tanstack/react-query';
import { client } from '@/api/client';
import { queryAuthorityAllows } from '@/api/queryAuthority';

export interface EnvironmentMcpChange { environmentMcp: string }

/** Native names and enablement are shared, while each thread supplies its own connection state. */
export async function repairEnvironmentMcp(cache: QueryClient, environment: string, current?: () => boolean): Promise<void> {
  const reader = client.session.getState().readerVersion;
  const sameReader = current ?? (() => client.session.getState().readerVersion === reader);
  const queries = cache.getQueryCache().getAll().filter((query) => query.queryKey[0] === 'environment-mcp' && query.queryKey[1] === environment);
  await Promise.all(queries.map(async (query) => {
    if (!sameReader()) return;
    if (!query.isActive()) { cache.removeQueries({ queryKey: query.queryKey, exact: true }); return; }
    await cache.cancelQueries({ queryKey: query.queryKey, exact: true });
    if (!sameReader() || cache.getQueryCache().find({ queryKey: query.queryKey, exact: true }) !== query
      || !query.isActive() || !queryAuthorityAllows(cache, query.meta)) return;
    await cache.refetchQueries({ queryKey: query.queryKey, exact: true });
  }));
}
