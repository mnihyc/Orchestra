/** Only shown native resource projections own their object streams. */
import type { QueryClient } from '@tanstack/react-query';
import { record } from '@/api/cache';
import { ENROLLMENT_PROGRESS } from '@/api/enrollment';

const RESOURCE_FEEDS: Readonly<Record<string, string>> = {
  provider: 'provider', agent: 'agent', host: 'host', machine: 'machine', template: 'template', 'mcp-server': 'mcp_server', mount: 'mount', skill: 'skill',
};

export function wantedResourceFeeds(cache: QueryClient): Set<string> {
  const wanted = new Set<string>();
  for (const query of cache.getQueryCache().getAll()) {
    if (!query.isActive()) continue;
    const [head, id] = query.queryKey;
    const data = query.state.data;
    // Native thread parameters can carry the provider facts shown by model/status choices.
    // The provider's own admission follows that embedded projection, independently of the thread.
    if (head === 'thread-parameters' && record(data) && record(data.provider)) {
      const provider = data.provider;
      if (typeof provider.id === 'string' && record(provider.access) && Array.isArray(provider.access.permissions)
        && provider.access.permissions.includes('provider.view')) wanted.add(`provider:${provider.id}`);
    }
    if (head === ENROLLMENT_PROGRESS && record(data)) {
      for (const kind of ['machine', 'host'] as const) {
        const row = data[kind];
        if (record(row) && typeof row.id === 'string') wanted.add(`${kind}:${row.id}`);
      }
    }
    if ((head === 'machines' || head === 'hosts') && Array.isArray(data)) {
      for (const row of data as unknown[]) {
        if (record(row) && typeof row.id === 'string') wanted.add(`${head === 'machines' ? 'machine' : 'host'}:${row.id}`);
      }
    }
    if (typeof head !== 'string' || typeof id !== 'string') continue;
    if (head === 'provider-login') wanted.add(`provider:${id}`);
    const kind = RESOURCE_FEEDS[head];
    if (kind !== undefined) wanted.add(`${kind}:${id}`);
  }
  return wanted;
}
