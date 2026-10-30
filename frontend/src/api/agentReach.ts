/** Agent reach has its own native projection; omitted changes repair only a related, shown environment. */
import type { QueryClient, QueryKey } from '@tanstack/react-query';
import { record } from '@/api/cache';
import { pathTo, useTreeStore } from '@/realtime/stores/tree';

export const agentReachKey = (environment: string) => ['agent-reach', environment] as const;
export const AGENT_REACH = agentReachKey('')[0];

/** A write's real target, never the changed node reused as an environment identifier. */
export type AgentReachDependency =
  | { scope: 'environment'; id: string }
  | { scope: 'node'; id: string; setting: string }
  | { scope: 'system'; setting: string };
export interface AgentReachChange { agentReach: AgentReachDependency }

const refId = (value: unknown): string | undefined => record(value) && typeof value.id === 'string' ? value.id : undefined;

function below(cache: QueryClient, home: string, ancestor: string): boolean {
  if (home === ancestor) return true;
  const held = cache.getQueryData(['node', home]);
  if (record(held) && Array.isArray(held.path) && held.path.some((place: unknown) => refId(place) === ancestor)) return true;
  for (const tree of useTreeStore.getState().byOrg.values()) {
    if (pathTo(tree, home).some((place) => place.id === ancestor)) return true;
  }
  return false;
}

/** Use only native homes, sources and already-held ancestry; this never discovers or grants access. */
export function agentReachProjections(cache: QueryClient, changed: AgentReachDependency): QueryKey[] {
  return cache.getQueryCache().getAll().filter((query) => {
    const [head, id] = query.queryKey;
    if (head !== AGENT_REACH || typeof id !== 'string') return false;
    if (changed.scope === 'environment') return changed.id === id;
    const overview = query.state.data;
    // Hidden secret names are no displayed setting to repair. A denial remains a denial.
    if (changed.setting === 'env.secret_names' && record(overview) && record(overview.secrets) && overview.secrets.names === null) return false;
    if (changed.scope === 'system') return true;
    if (record(overview) && Array.isArray(overview.home_path) && overview.home_path.some((place: unknown) => refId(place) === changed.id)) return true;
    const environment = cache.getQueryData(['environment', id]);
    if (record(environment) && environment.org_id === changed.id) return true;
    const homes: string[] = [];
    if (record(environment)) {
      const home = refId(environment.home);
      if (home !== undefined) homes.push(home);
    }
    if (record(overview)) {
      const secretHome = record(overview.secrets) ? refId(overview.secrets.source) : undefined;
      if (secretHome !== undefined) homes.push(secretHome);
      if (record(overview.network) && Array.isArray(overview.network.sources)) {
        for (const source of overview.network.sources as unknown[]) {
          const home = record(source) ? refId(source.scope) : undefined;
          if (home !== undefined) homes.push(home);
        }
      }
    }
    return homes.some((home) => below(cache, home, changed.id));
  }).map((query) => query.queryKey);
}
