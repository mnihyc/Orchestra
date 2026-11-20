/** Whole resource rows already supplied by the current reader's native detail and list queries. */
import type { QueryCache, QueryClient } from '@tanstack/react-query';
import type { Agent, Environment, Host, Machine, McpServer, Mount, Provider, Skill, Template } from '@/generated/api-types';

interface Resources {
  agent: Agent; environment: Environment; host: Host; machine: Machine; mcp_server: McpServer;
  mount: Mount; provider: Provider; skill: Skill; template: Template;
}
type Kind = keyof Resources;
type Row = Record<string, unknown>;
interface ResourceRead { readonly queryHash: string; readonly queryKey: readonly unknown[]; readonly state: { readonly data: unknown } }
export const record = (value: unknown): value is Row => typeof value === 'object' && value !== null && !Array.isArray(value);

/** Table projections and providers with a truncated model list cannot stand for a detail page. */
export function wholeRow(kind: string, row: Row): boolean {
  if (row.list_row === true) return false;
  return kind !== 'provider' || typeof row.models_count !== 'number' || Array.isArray(row.models) && row.models.length >= row.models_count;
}

const nativeKinds = new Map<string, Kind>([
  ['agent', 'agent'], ['agents', 'agent'], ['environment', 'environment'], ['environments', 'environment'],
  ['host', 'host'], ['hosts', 'host'], ['machine', 'machine'], ['machines', 'machine'],
  ['mcp-server', 'mcp_server'], ['mcp-servers', 'mcp_server'], ['mount', 'mount'], ['mounts', 'mount'],
  ['provider', 'provider'], ['providers', 'provider'], ['skill', 'skill'], ['skills', 'skill'],
  ['template', 'template'], ['templates', 'template'],
]);
const everythingRoots = new Set(['everything', 'provider-default-choices', 'resource-navigation']);
const arrays = (value: unknown): readonly unknown[] => Array.isArray(value) ? value : [];

/** Follow the API's resource fields. Names, private list rows and arbitrary payloads are never detail answers. */
function suppliedRows(query: ResourceRead, put: (kind: Kind, id: string, row: Row) => void): void {
  const [root, second] = query.queryKey;
  const data = query.state.data;
  if (typeof root !== 'string' || data === undefined) return;
  const add = (kind: Kind, row: unknown) => {
    if (record(row) && typeof row.id === 'string' && row.content_hidden !== true && record(row.access) && wholeRow(kind, row)) put(kind, row.id, row);
  };
  if (record(data)) {
    if (root === 'enrollment-progress') { add('machine', data.machine); add('host', data.host); return; }
    if (root === 'machine-programs' || root === 'machine-policy') { add('machine', data.machine_view); return; }
  }
  const kind = root === 'admin' && second === 'hosts' ? 'host' : nativeKinds.get(root);
  if (kind === undefined && !everythingRoots.has(root)) return;
  const addItem = (item: unknown) => {
    if (!record(item) || item.content_hidden === true) return;
    if (record(item.resource)) {
      const { kind: tag, value } = item.resource;
      const resourceKind = tag === 'mcp_server' ? tag : typeof tag === 'string' ? nativeKinds.get(tag) : undefined;
      if (resourceKind !== undefined && tag === resourceKind && (kind === undefined || kind === resourceKind)
        && record(value) && value.id === item.id) add(resourceKind, value);
    } else if (kind !== undefined && !('resource' in item) && !('resource_row' in item)) add(kind, item);
  };
  if (record(data) && Array.isArray(data.pages)) {
    for (const page of arrays(data.pages)) if (record(page)) arrays(page.items).forEach(addItem);
  } else if (record(data) && Array.isArray(data.items)) {
    arrays(data.items).forEach(addItem);
  } else if (Array.isArray(data)) arrays(data).forEach(addItem);
  else addItem(data);
}

type FindRow = (key: string) => Row | undefined;
const indexes = new WeakMap<QueryCache, FindRow>();

export function resourceRowIndex(cache: QueryCache): FindRow {
  const existing = indexes.get(cache);
  if (existing !== undefined) return existing;
  const rows = new Map<string, Map<ResourceRead, Row>>();
  const sources = new WeakMap<ResourceRead, { data: unknown; keys: Set<string> | undefined }>();
  const update = (query: ResourceRead, removed: boolean) => {
    const previous = sources.get(query);
    if (!removed && previous?.data === query.state.data) return;
    let keys: Set<string> | undefined;
    if (!removed) suppliedRows(query, (kind, id, row) => {
      const key = `${kind}:${id}`;
      (keys ??= new Set()).add(key);
      let answers = rows.get(key);
      if (answers === undefined) { answers = new Map(); rows.set(key, answers); }
      answers.set(query, row);
    });
    if (previous?.keys !== undefined) for (const key of previous.keys) {
      if (keys?.has(key) === true) continue;
      const answers = rows.get(key);
      answers?.delete(query);
      if (answers?.size === 0) rows.delete(key);
    }
    if (removed) sources.delete(query);
    else sources.set(query, { data: query.state.data, keys });
  };
  for (const query of cache.getAll()) update(query, false);
  cache.subscribe((event) => {
    if (event.type === 'removed') update(event.query, true);
    else if ((event.type === 'added' || event.type === 'updated') && cache.get(event.query.queryHash) === event.query) update(event.query, false);
  });
  const find: FindRow = (key) => {
    const answers = rows.get(key);
    if (answers === undefined) return undefined;
    for (const [source, row] of answers) {
      const removed = cache.get(source.queryHash) !== source;
      if (!removed && sources.get(source)?.data === source.state.data) return row;
      // Query observers can read synchronously before QueryCache publishes this replacement.
      update(source, removed);
      const current = answers.get(source);
      if (current !== undefined) return current;
    }
    return undefined;
  };
  indexes.set(cache, find);
  return find;
}

export function cachedRow<K extends Kind>(cache: QueryClient, kind: K, id: string): Resources[K] | undefined {
  if (id === '') return undefined;
  const find = resourceRowIndex(cache.getQueryCache());
  return find(`${kind}:${id}`) as Resources[K] | undefined;
}
