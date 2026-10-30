/** Source replacement follows the canonical environment query, once for all panes and file tabs. */
import type { QueryClient } from '@tanstack/react-query';
import { ApiError } from '@/api/client';
import type { Environment } from '@/generated/api-types';

function record(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}
function queryKey(value: unknown): value is readonly unknown[] { return Array.isArray(value); }
interface ObservedQuery { queryKey: unknown; state: { data: unknown; error: unknown } }

function binding(value: unknown): string | null {
  if (!record(value) || typeof value.id !== 'string') return null;
  if (typeof value.workspace_incarnation === 'string') return JSON.stringify([value.id, value.workspace_incarnation]);
  // Older projections have no opaque incarnation. This equality fallback cannot detect a
  // replacement hidden by redaction; native cursor and watch fences still apply to those readers.
  if (typeof value.generation !== 'number' || typeof value.workspace_root !== 'string') return null;
  const source = record(value.data_sources) ? value.data_sources.workspace : undefined;
  const selected = !record(source) ? ['unspecified'] : source.kind === 'existing_directory'
    ? ['existing_directory', source.path] : ['made_by_orch'];
  return JSON.stringify(['workspace-source', value.id, value.generation, value.workspace_root, selected,
    record(value.machine) ? value.machine.id : null, record(value.host) ? value.host.id : null]);
}

export function workspaceBinding(environment: Environment): string {
  // Every Environment has these required identity fields, including older redacted projections.
  return binding(environment) ?? JSON.stringify(['unknown-workspace', environment.id]);
}

export function installWorkspaceBindings(cache: QueryClient, forget: (environmentId: string) => void): () => void {
  const accepted = new WeakMap<object, string>();
  const observe = (query: ObservedQuery) => {
    const key = query.queryKey;
    if (!queryKey(key)) return;
    const [kind, id] = key;
    if (kind !== 'environment' || typeof id !== 'string' || key.length !== 2) return;
    const next = binding(query.state.data), before = accepted.get(query);
    if (query.state.error instanceof ApiError && (query.state.error.status === 403 || query.state.error.status === 404)) {
      if (before !== undefined) { accepted.delete(query); forget(id); }
      return;
    }
    if (next === null) return;
    if (before !== undefined && before !== next) forget(id);
    accepted.set(query, next);
  };
  for (const query of cache.getQueryCache().getAll()) observe(query);
  return cache.getQueryCache().subscribe((event) => {
    if (event.type === 'removed') {
      const key: unknown = event.query.queryKey;
      if (!queryKey(key)) return;
      const [kind, id] = key;
      if (kind === 'environment' && typeof id === 'string' && accepted.has(event.query)) forget(id);
      accepted.delete(event.query); return;
    }
    if (event.type === 'added' || event.type === 'updated') observe(event.query);
  });
}
