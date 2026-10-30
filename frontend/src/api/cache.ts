/** Apply authorized rows to their detail, embedded references and loaded list pages without another request. */
import type { QueryClient, QueryKey } from '@tanstack/react-query';
import { matches as matchesName } from '@/features/finder/ranking';
import { ApiError } from '@/api/client';
import type { Agent, Environment, Host, Machine, McpServer, Mount, Provider, Skill, Template, Thread } from '@/generated/api-types';
import { isThreadForScreen } from '@/features/thread/cached';

/** Cursors obtained with authoritative HTTP snapshots, cleared with the human session. */
export const feedSequences = new Map<string, number>();

export interface RowChange {
  row: {
    kind: string;
    id: string;
    /** Null removes the row. A non-null value is the API's authorized DTO, never a guessed row. */
    value: Record<string, unknown> | null;
    /** Only a creation or a complete live row can enter a list where it was not already present. */
    insert?: boolean;
    /** Tree summaries update existing fields and cannot seed a detail or add a rich list row. */
    partial?: boolean;
    parent?: string;
  };
}

export interface ValueChange {
  value: { key: QueryKey; data: unknown };
}

/** Only a complete native Thread can replace the conversation view; generic rows are metadata. */
export interface ThreadChange { thread: Thread }

export function isCompleteThread(value: unknown): value is Thread {
  if (!record(value) || typeof value.id !== 'string' || !isThreadForScreen(value, value.id) || !record(value.can)) return false;
  const can = value.can;
  return (['archive', 'export', 'fork', 'place'] as const).every((key) => typeof can[key] === 'boolean');
}

export interface CollectionChange {
  collection: { key: QueryKey; identity: string; id: string; data: unknown; field?: string };
  /** An accepted write must retire a list read that began before it. */
  cancelPending?: true;
}

/** One returned model/setting belongs to one parent's collection, never every collection of its kind. */
export function applyCollection(cache: QueryClient, change: CollectionChange['collection']): void {
  cache.setQueryData(change.key, (held: unknown) => {
    const items=change.field===undefined ? held : record(held) ? held[change.field] : undefined;
    if (!Array.isArray(items)) return held;
    const rows = items as unknown[];
    const found = rows.some((one) => record(one) && one[change.identity] === change.id);
    const next = rows.flatMap((one) => record(one) && one[change.identity] === change.id ? change.data === null ? [] : [change.data] : [one]);
    const updated=found || change.data === null ? next : [...next, change.data];
    return change.field!==undefined && record(held) ? {...held,[change.field]:updated} : updated;
  });
}

export const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const array = (value: unknown): value is unknown[] => Array.isArray(value);

/** Only these native DTOs can seed their corresponding resource detail readers. */
interface CachedResources {
  agent: Agent;
  environment: Environment;
  host: Host;
  machine: Machine;
  mcp_server: McpServer;
  mount: Mount;
  provider: Provider;
  skill: Skill;
  template: Template;
}

const LISTS: Readonly<Record<string, readonly string[]>> = {
  node: ['children', 'topics', 'orgs', 'shared-with-me'],
  provider: ['providers'], agent: ['agents'], host: ['hosts'], environment: ['environments'],
  machine: ['machines'], template: ['templates'], mcp_server: ['mcp-servers'], mount: ['mounts'], skill: ['skills'],
  document: ['documents'], tag: ['tags'], group: ['groups', 'org-groups'],
  approval: ['approvals', 'port-requests'], notification_channel: ['notification-channels'],
  export: ['exports'], job: [], thread: ['thread-sides', 'provider-threads'],
  api_token: ['tokens','org-tokens'], browser: ['environment-browsers'], display: ['environment-displays'],
};
const DETAILS: Readonly<Record<string, string>> = { mcp_server: 'mcp-server', notification_channel: 'notification-channel' };
/** These are stored facts or user-written JSON, never live entity-reference projections. */
const OPAQUE_MEMBERS=new Set(['config','runtime_config','params','parameters','spec','raw','payload','body','content','data','facts']);

function overlay(before: Record<string, unknown>, row: Record<string, unknown>): Record<string, unknown> {
  // A name/reference stays a name/reference. Do not turn a path-only row into a rich resource.
  const next = { ...before };
  for (const key of Object.keys(before)) if (key in row) {
    const old = before[key]; const value = row[key];
    next[key] = key !== 'access' && record(old) && record(value) ? overlay(old, value) : value;
  }
  return next;
}

/** References and node summaries may rename a thread, never overwrite its capabilities or binding. */
function threadMetadata(before: Record<string, unknown>, row: Record<string, unknown>): Record<string, unknown> {
  if (row.redacted === true) return before;
  const next = { ...before };
  for (const key of ['name', 'description']) if (typeof row[key] === 'string') next[key] = row[key];
  if (typeof row.archived === 'boolean') next.archived = row.archived;
  if ('color' in row && (row.color === null || typeof row.color === 'number')) next.color = row.color;
  return next;
}

/** Inventory state follows the list projection; registration status alone does not say a machine is online. */
function inventoryState(kind: string, row: Record<string, unknown>): unknown {
  if (kind === 'machine') {
    if (row.status === 'revoked') return 'revoked';
    return typeof row.online === 'boolean' ? (row.online ? 'online' : 'offline') : undefined;
  }
  return row.status ?? row.state;
}

/** Replace matching references recursively, preserving the shape and paging metadata of each projection. */
function replace(before: unknown, change: RowChange['row'], thread?: Thread): unknown {
  if (Array.isArray(before)) {
    const next = before.flatMap((item: unknown) => record(item) && item.id === change.id && change.value === null ? [] : [replace(item, change, thread)]);
    // ResourceRows keeps private names as a non-index property of its readable array.
    if ('privateRows' in before && Array.isArray(before.privateRows)) return Object.assign(next, { privateRows: replace(before.privateRows, change, thread) });
    return next;
  }
  if (!record(before)) return before;
  let next = before;
  if (before.id === change.id) {
    // A separately authorized thread can still name its bound provider. Revoking the rich
    // provider view removes that view, not a required name/reference inside another DTO.
    if (change.value === null) return 'access' in before || 'resource' in before ? null : before;
    const complete=change.partial!==true && before.content_hidden!==true && record(before.access) && Array.isArray(before.access.permissions) && !record(before.resource);
    const nameOnly = INVENTORY_KINDS.has(change.kind) && 'my_access' in before && 'path' in before && !record(before.resource);
    if (nameOnly) {
      // Preserve the inventory's withheld metadata and native policy/path, even when the detail knows more.
      next = { ...before };
      if (typeof before.name === 'string' && typeof change.value.name === 'string') next.name = change.value.name;
      const state = inventoryState(change.kind, change.value);
      if (typeof before.state === 'string' && typeof state === 'string') next.state = state;
    } else next = before.kind === 'thread' && record(before.access)
      ? thread === undefined ? threadMetadata(before, change.value) : { ...thread }
      : complete ? {...change.value} : overlay(before, change.value);
    // Inventory kind names the object; a provider/template DTO's kind names its subtype.
    if ('my_access' in before && 'path' in before && typeof before.kind === 'string') next.kind = before.kind;
    // Everything wraps the native DTO in a tagged resource, with its own name/state columns.
    if (record(before.resource) && before.resource.kind === change.kind && change.partial !== true) {
      next = { ...next, resource: { ...before.resource, value: change.value } };
      if ('state' in before) next.state = inventoryState(change.kind, change.value) ?? before.state;
    }
  }
  const result = { ...next };
  for (const [key, value] of Object.entries(next)) {
    if (key === 'resource' && before.id === change.id && record(value) && value.kind === change.kind) continue;
    if(OPAQUE_MEMBERS.has(key) || key==='value' && !(typeof next.kind==='string' && next.kind in LISTS)) continue;
    if (Array.isArray(value) || record(value)) result[key] = replace(value, change, thread);
  }
  return result;
}

/** Whether a row still meets the loaded resource list's explicit filters. Unknown facts never grant entry. */
function matches(key: QueryKey, kind: string, row: Record<string, unknown>, known: boolean): boolean {
  if (kind === 'group' && (key[0] === 'groups' || key[0] === 'org-groups')) return key[1] === row.org_id || known;
  if (key[0] === 'hosts' && key.length === 2 && typeof key[1] === 'string' && row.org_id !== key[1]) return false;
  if (key[0] === 'orgs' && (row.kind !== 'org' || row.parent_id !== null)) return false;
  if ((key[0] === 'children' || key[0] === 'topics') && row.parent_id !== key[1]) return false;
  if (key[0] === 'topics' && row.kind !== 'topic') return false;
  if (key[0] === 'provider-threads' && (!record(row.binding) || !record(row.binding.provider) || row.binding.provider.id !== key[1])) return false;
  if (key[0] === 'thread-sides' && (!record(row.side_of) || row.side_of.id !== key[1])) return false;
  if (key[0] === 'shared-with-me' && !known) return false;
  if (key[0] === 'documents') {
    if (!record(row.node) || typeof row.node.id !== 'string' || typeof row.deleted !== 'boolean') return known;
    if (row.deleted !== (key[3] === true)) return false;
    const here = row.node.id === key[1];
    if (!here && key[2] !== true) return false;
    if (!here && Array.isArray(row.path) && !row.path.some((part: unknown) => record(part) && part.id === key[1])) return false;
    if (!here && !Array.isArray(row.path) && !known) return false;
    if (key[4] === true) {
      const participation = record(row.access) ? row.access.participating : row.participating;
      if (participation !== true) return participation === undefined && known;
    }
    return true;
  }
  if (key[0] === 'providers' && typeof key[1] === 'string' && ['mine', 'usable', 'managed'].includes(key[1])) {
    const access = record(row.access) ? row.access : {};
    const owns = Array.isArray(access.through) && access.through.some((entry: unknown) => record(entry) && entry.kind === 'owner');
    if (key[1] === 'mine' && !owns) return false;
    if (key[1] === 'usable' && (!Array.isArray(access.permissions) || !access.permissions.includes('provider.use'))) return false;
    if (key[1] === 'managed' && (!Array.isArray(access.permissions) || !access.permissions.includes('provider.manage'))) return false;
    if (typeof key[2] === 'string' && row.org_id !== key[2] && (!record(row.home) || row.home.id !== key[2])) return known;
    return true;
  }
  const filter = key.findLast((part) => record(part));
  if (!record(filter)) return known || key.length <= 2;
  if (typeof filter.q === 'string' && filter.q.trim() !== '') {
    if (!matchesName(filter.q, typeof row.name === 'string' ? row.name : '')) return false;
  }
  if (typeof filter.owner === 'string') {
    const owner = record(row.owner) ? row.owner.id : row.owner_id;
    if (owner !== filter.owner) return false;
  }
  if (filter.participating === true && row.participating !== true && !known) return false;
  const resource = record(filter.resource) ? filter.resource : filter;
  for (const [predicate, field] of [['state', 'status'], ['harness', 'harness_kind'], ['kind', 'kind']] as const) {
    const actual = predicate === 'state' && inventory(key, kind) ? inventoryState(kind, row) : row[field] ?? row[predicate];
    if (typeof resource[predicate] === 'string' && (actual === undefined ? !known : resource[predicate] !== actual)) return false;
  }
  // A home filter may name an ancestor omitted from the native row. Keep the server's known
  // membership on a rename; an unknown row cannot gain entry without a matching home/root.
  if (typeof resource.home === 'string' && (!record(row.home) || row.home.id !== resource.home) && row.org_id!==resource.home) return known;
  if (!known && resource.view === 'mine' && (!record(row.owner) || row.owner.id !== key[1])) return false;
  if (!known && resource.view === 'managed' && (!record(row.access) || row.access.administers !== true)) return false;
  // A new native row cannot reconstruct redacted paths, parent placement or an owner's labels.
  // The subscribed inventory feed supplies that projection when required.
  return known || key[2] === 'resources' || !('scope' in filter);
}

const INVENTORY_KINDS=new Set(['provider','agent','environment','host','machine','template','mcp_server','mount','skill']);

function inventory(key:QueryKey,kind:string):boolean {
  return INVENTORY_KINDS.has(kind) && key[2]==='resources' && record(key[3]) && Array.isArray(key[3].kinds) && (key[3].kinds.length===0 || key[3].kinds.includes(kind));
}

/** The ordinary resource DTO omits inventory policy/path fields. Do not guess them on insertion
 * or a changed home/access: one exact visible projection can supply the missing native row.
 */
export function missingInventoryProjection(cache:QueryClient,change:RowChange['row'], previous?: { row: unknown }):QueryKey|null {
  if(change.value===null || change.partial===true || !INVENTORY_KINDS.has(change.kind)) return null;
  const cut = (value: Record<string, unknown>) => JSON.stringify([value.access, value.admin_access, value.participating, value.home, value.org_id, value.kind]);
  const prior = previous === undefined ? cache.getQueryData([DETAILS[change.kind] ?? change.kind, change.id]) : previous.row;
  const sameCut = record(prior) && cut(prior) === cut(change.value);
  const find=(value:unknown):Record<string,unknown>|undefined=>{
    if(Array.isArray(value)) {for(const item of value as unknown[]) {const row=find(item);if(row!==undefined)return row;}}
    else if(record(value)) {
      if(value.id===change.id && (value.kind===change.kind || record(value.resource) && value.resource.kind===change.kind))return value;
      for(const [key,item] of Object.entries(value)) if(!OPAQUE_MEMBERS.has(key) && (Array.isArray(item)||record(item))) {const row=find(item);if(row!==undefined)return row;}
    }
    return undefined;
  };
  const detailed=(key:QueryKey)=>record(key[3]) && record(key[3].resource) && key[3].resource.details===true;
  const queries=[...cache.getQueryCache().getAll()].sort((one,other)=>Number(detailed(other.queryKey))-Number(detailed(one.queryKey)));
  let visible:QueryKey|null=null;
  for(const query of queries) {
    if(!inventory(query.queryKey,change.kind))continue;
    const row=find(query.state.data);
    const value=row!==undefined && record(row.resource) && record(row.resource.value) ? row.resource.value : undefined;
    // A known ordinary name row retains its native path/policy during state/name updates.
    // Private/path-only rows, new membership and changed authority still need the exact inventory cut.
    const knownName = row !== undefined && typeof row.name === 'string' && typeof row.my_access === 'string'
      && ['view', 'take_part', 'manage'].includes(row.my_access) && !detailed(query.queryKey);
    const missing=row===undefined ? change.insert===true : value===undefined ? !(knownName && sameCut) : cut(value)!==cut(change.value);
    if(!missing)continue;
    if(query.isActive())visible ??=query.queryKey;
    else cache.removeQueries({queryKey:query.queryKey,exact:true});
  }
  return visible;
}

function contains(value: unknown, id: string): boolean {
  if (Array.isArray(value)) return value.some((one: unknown) => contains(one, id));
  return record(value) && (value.id === id || Object.values(value).some((one) => (Array.isArray(one) || record(one)) && contains(one, id)));
}

function listChange(before: unknown, key: QueryKey, change: RowChange['row'], thread?: Thread): unknown {
  const head = key[0];
  const isInventory = inventory(key,change.kind);
  const isList = isInventory || (typeof head === 'string' && (LISTS[change.kind] ?? []).includes(head));
  if (!isList) return replace(before, change, thread);
  const known = contains(before, change.id);
  const accepted = change.value !== null && !(change.kind === 'environment' && change.value.status === 'removed') && matches(key, change.kind, change.value, known);
  const next = replace(before, accepted ? change : { ...change, value: null }, thread);
  if (known || !accepted || change.insert !== true || change.value === null) return next;
  if(isInventory) return next;
  if (change.parent !== undefined && !key.includes(change.parent) && change.kind !== 'document') return next;
  if (array(next)) {
    const rows = [...next, change.value];
    return 'privateRows' in next ? Object.assign(rows, { privateRows: next.privateRows }) : rows;
  }
  if (!record(next)) return next;
  if (array(next.items)) return { ...next, items: [change.value, ...next.items] };
  if (array(next.pages)) {
    const item = change.value;
    const first: unknown = next.pages[0];
    if (!record(first) || !array(first.items)) return next;
    return { ...next, pages: [{ ...first, items: [item, ...first.items] }, ...next.pages.slice(1)] };
  }
  return next;
}

/** One native row updates every loaded projection of that identity, including inactive cached pages. */
export function applyRow(cache: QueryClient, change: RowChange['row']): void {
  writeRow(cache, change);
}

export function applyThread(cache: QueryClient, thread: Thread): void {
  writeRow(cache, { kind: 'thread', id: thread.id, value: { ...thread }, insert: true }, thread);
}

function writeRow(cache: QueryClient, change: RowChange['row'], thread?: Thread): void {
  const detail = DETAILS[change.kind] ?? change.kind;
  const held = cache.getQueryData([detail, change.id]);
  const threadView = detail === 'thread' || change.value?.kind === 'thread' || record(held) && held.kind === 'thread';
  if (change.value !== null) {
    for (const before of [held, thread === undefined ? undefined : cache.getQueryData(['node', thread.id])]) {
      if (!record(before)) continue;
      for (const key of ['revision', 'seq', 'seq_updated']) {
      const old = before[key]; const next = change.value[key];
      if (typeof old === 'number' && typeof next === 'number' && old > next) return;
      }
    }
  }
  // A machine report carries its live registry too. Only update an already-open Programs
  // projection, using the API row's actual authority rather than the native report's flag.
  if(change.kind==='machine' && change.partial!==true) {
    const query=cache.getQueryCache().find({queryKey:['machine-programs',change.id],exact:true});
    if(query!==undefined) {
      if(change.value===null) {
        if(!query.isActive())cache.removeQueries({queryKey:query.queryKey,exact:true});
        else query.setState({data:undefined,error:new ApiError({type:'orch:error:not_found',code:'not_found',title:'',detail:'',status:404,request_id:''},null),status:'error',fetchStatus:'idle'});
      }else {
        const facts=record(change.value.facts)?change.value.facts:{};
        const reported=record(facts.programs)?facts.programs:{entries:[],capabilities:[],config_file:null};
        const access=record(change.value.access)?change.value.access:{};
        const canManage=Array.isArray(access.permissions) && access.permissions.some((permission:unknown)=>permission==='machine.manage' || permission==='host.manage');
        cache.setQueryData(query.queryKey,{...reported,can_manage:canManage});
      }
    }
  }
  for (const query of cache.getQueryCache().getAll()) {
    const key = query.queryKey;
    if (key[0] === detail && key[1] === change.id && key.length === 2) {
      if (change.value === null) {
        if (!query.isActive()) cache.removeQueries({ queryKey: key, exact: true });
        else query.setState({ data: undefined, error: new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: '', detail: '', status: 404, request_id: '' }, null), status: 'error', fetchStatus: 'idle' });
      }
      else if (threadView || record(query.state.data) && query.state.data.kind === 'thread') {
        if (thread !== undefined) cache.setQueryData(key, thread);
        else if (record(query.state.data)) cache.setQueryData(key, threadMetadata(query.state.data, change.value));
      } else if(change.partial===true) {
        if(record(query.state.data)) cache.setQueryData(key,overlay(query.state.data,change.value));
      } else cache.setQueryData(key,change.value);
    } else if (query.state.data !== undefined) {
      const relatedList = typeof key[0] === 'string' && (LISTS[change.kind] ?? []).includes(key[0]) || key[2] === 'resources';
      if (!relatedList && !contains(query.state.data, change.id)) continue;
      cache.setQueryData(key, listChange(query.state.data, key, change, thread));
    }
  }
  if (thread !== undefined) cache.setQueryData(['node', thread.id], thread);
  else if (!threadView && change.value !== null && change.partial !== true) cache.setQueryData([detail, change.id], change.value);
}

/** Use a list's already-authorized native row before asking the API for the same detail. */
export function cachedRow<Kind extends keyof CachedResources>(cache: QueryClient, kind: Kind, id: string): CachedResources[Kind] | undefined {
  const find = (value: unknown): Record<string, unknown> | undefined => {
    if (Array.isArray(value)) {
      for (const one of value as unknown[]) { const found = find(one); if (found !== undefined) return found; }
    } else if (record(value)) {
      if (value.id === id && value.content_hidden !== true && record(value.resource) && value.resource.kind === kind && record(value.resource.value)) return value.resource.value;
      if (value.id === id && value.content_hidden !== true && record(value.access) && !('resource' in value)) return value;
      for (const one of Object.values(value)) if (Array.isArray(one) || record(one)) { const found = find(one); if (found !== undefined) return found; }
    }
    return undefined;
  };
  for (const query of cache.getQueryCache().getAll()) {
    const found = find(query.state.data);
    if (found !== undefined) return found as unknown as CachedResources[Kind];
  }
  return undefined;
}
