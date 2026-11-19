/** Page-read bookkeeping uses the fake's native personal mutators, without another HTTP request. */
import type { PersonalTarget } from '../../src/generated/api-types';
import type { Person, World } from './world';
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const kinds: Readonly<Record<string, PersonalTarget['kind']>> = { org: 'org', project: 'project', topic: 'topic', thread: 'thread', doc: 'document', providers: 'provider', agents: 'agent', environments: 'environment', machines: 'machine', hosts: 'host', templates: 'template', mcp: 'mcp_server', mounts: 'mount' };

export async function noteView(world: World, person: Person, route: string, value: unknown): Promise<void> {
  if (!record(value)) return;
  const path = route.split('?')[0] ?? '';
  const parts = path.split('/').filter(Boolean);
  let row: unknown = value.thread ?? value.document ?? (record(value.sync) ? value.sync.thread : undefined) ?? value;
  const category = parts[0] === 'resources' ? parts[1] : parts[0];
  const kind = category === undefined ? undefined : kinds[category];
  if (kind === undefined) return;
  const target = parts[parts[0] === 'resources' ? 2 : 1];
  if (kind === 'org' && Array.isArray(value.orgs)) row = (value.orgs as unknown[]).find((one) => record(one) && one.address === target);
  if (!record(row) || typeof row.id !== 'string' || !record(row.access) || row.content_hidden === true) return;
  if (kind === 'org' ? row.address !== target : row.id !== target) return;
  async function invoke(method: string, path: string, body: Record<string, unknown>) {
    for (const { part } of world.parts) {
      const answer = await part.handle?.(person, method, path, new URLSearchParams(), body);
      if (answer !== null && answer !== undefined) return answer.body;
    }
    return undefined;
  }
  await invoke('POST', '/me/visits', { kind, id: row.id });
  const node = ['org', 'project', 'topic', 'thread'].includes(kind) ? row.id : kind === 'document' && record(row.node) ? row.node.id : undefined;
  if (typeof node !== 'string') return;
  const state = await invoke('GET', '/me/state', {});
  const sidebar = record(state) && record(state.sidebar) ? state.sidebar : {};
  await invoke('PUT', '/me/state/sidebar', { ...sidebar, last_place: { node_id: node, route }, ...(kind === 'org' ? { last_org_id: row.id } : typeof row.org_id === 'string' ? { last_org_id: row.org_id } : {}) });
}
