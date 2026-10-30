/** Exact-object repair for older live events, plus an exact inventory projection if its policy fields are omitted. */
import type { QueryClient } from '@tanstack/react-query';
import { api, ApiError, client, ok } from '@/api/client';
import { applyRow, missingInventoryProjection, record } from '@/api/cache';
import { resourceFollowing } from '@/realtime/resourceFollowing';

export interface ResourceRepair { repair: { kind: string; id: string; source?: 'news' } }
const LISTS: Readonly<Record<string, string>> = { provider: 'providers', agent: 'agents', environment: 'environments', host: 'hosts', machine: 'machines', template: 'templates', mcp_server: 'mcp-servers', mount: 'mounts', skill: 'skills' };

async function read(kind: string, id: string, signal: AbortSignal): Promise<unknown> {
  const params = { path: { id } };
  switch (kind) {
    case 'provider': return ok(api.GET('/providers/{id}', { params, signal }));
    case 'agent': return ok(api.GET('/agents/{id}', { params, signal }));
    case 'environment': return ok(api.GET('/environments/{id}', { params, signal }));
    case 'host': return ok(api.GET('/hosts/{id}', { params, signal }));
    case 'machine': return ok(api.GET('/machines/{id}', { params, signal }));
    case 'template': return ok(api.GET('/templates/{id}', { params, signal }));
    case 'mcp_server': return ok(api.GET('/mcp-servers/{id}', { params, signal }));
    case 'mount': return ok(api.GET('/mounts/{id}', { params, signal }));
    case 'skill': return ok(api.GET('/skills/{id}', { params, signal }));
    default: return undefined;
  }
}

export async function repairResource(cache: QueryClient, { kind, id, source }: ResourceRepair['repair']): Promise<void> {
  const reader = client.session.getState().readerVersion;
  const detail = kind === 'mcp_server' ? 'mcp-server' : kind;
  // General news precedes the native row on its object feed. That feed owns an already shown
  // detail (or known host/machine list member), including admission/revocation and initial state.
  if (source === 'news' && resourceFollowing(kind, id) && cache.getQueryCache().getAll().some((query) => query.isActive() && (
    query.queryKey[0] === detail && query.queryKey[1] === id
    || (kind === 'host' && query.queryKey[0] === 'hosts' || kind === 'machine' && query.queryKey[0] === 'machines')
      && Array.isArray(query.state.data) && query.state.data.some((row: unknown) => record(row) && row.id === id)
  ))) return;
  const shown = cache.getQueryCache().getAll().some((query) => {
    if (!query.isActive()) return false;
    const key = query.queryKey;
    if (key[0] === detail && key[1] === id || key[0] === LISTS[kind]) return true;
    const filter = key[3];
    return key[2] === 'resources' && record(filter) && Array.isArray(filter.kinds) && filter.kinds.includes(kind);
  });
  if (!shown) return;
  const previous = cache.getQueryData([detail, id]);
  try {
    const row = await cache.query({ queryKey: [detail, id], queryFn: ({ signal }) => read(kind, id, signal), staleTime: 0, retry: false });
    if (client.session.getState().readerVersion !== reader) return;
    if (record(row) && row.id === id) {
      const change={kind,id,value:row,insert:true};const projection=missingInventoryProjection(cache,change,{row:previous});
      applyRow(cache,change);
      if(projection!==null)await cache.refetchQueries({queryKey:projection,exact:true});
    }
  } catch (error) {
    if (client.session.getState().readerVersion !== reader) return;
    if (error instanceof ApiError && (error.status === 403 || error.status === 404)) applyRow(cache, { kind, id, value: null });
    // The exact query keeps any other failure; a later event or an explicit Retry may repair it.
  }
}
