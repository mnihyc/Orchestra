/** The complete inventory in the fake world, using its existing place, document, chat and resource readers. */
import type { EverythingAccess, EverythingKind, EverythingRow, NodeRef, UserRef } from '../../src/generated/api-types.ts';
import { MADE_AT } from './fixtures.ts';
import { inventoryRow } from './everything-row.ts';
import { webHref } from './destinations.ts';
import { matches as matchesQuery, matchScore, wordsOf } from '../../src/features/finder/ranking.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** The production contract's limits. */
const PAGE_DEFAULT = 50;
const PAGE_MAX = 200;

interface Position { relevance: number; visitScore: number; activity: string | null; key: string | number | null; kind: string; id: string }
interface Cursor extends Position { selection: string; scoreAt: number }

const ref = (step: { id: string; kind: NodeRef['kind']; name: string | null }): NodeRef => ({ ...step, redacted: step.name === null });
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null;
const human = (value: unknown): UserRef | null => isRecord(value) && typeof value.id === 'string' && typeof value.display_name === 'string' ? value as unknown as UserRef : null;
const level = (permissions: readonly string[], kind: string): EverythingAccess => permissions.includes(`${kind}.manage`) ? 'manage' : permissions.some((permission) => [`${kind}.interact`, `${kind}.edit`, `${kind}.use`].includes(permission)) ? 'take_part' : 'view';

export class EverythingWorld implements Part {
  private overrides = new Map<string, EverythingRow[]>();
  private readonly world: World;
  constructor(world: World) { this.world = world; }
  reset(): void { this.overrides.clear(); }

  private path(person: Person, id: string): NodeRef[] {
    const places = this.world.places;
    return places.pathTo(id).map((step) => {
      const root = places.rootOf(step.id);
      const named = places.access(person, step.id).permissions.length > 0 || (step.kind === 'org' && root !== null && places.isMember(person, root));
      return ref({ id: step.id, kind: step.kind, name: named ? step.name : null });
    });
  }

  private rows(person: Person, system: boolean): EverythingRow[] {
    const controlled = this.overrides.get(`${person.me.username}:${system ? 'system' : 'mine'}`) ?? this.overrides.get(`${person.me.username}:all`);
    if (controlled !== undefined) return structuredClone(controlled);
    const places = this.world.places;
    const rows = new Map<string, EverythingRow>();
    const addAncestors = (path: readonly NodeRef[]) => {
      path.forEach((step, index) => {
        if (rows.has(step.id)) return;
        rows.set(step.id, inventoryRow({ id: step.id, kind: step.kind, name: step.name, path: path.slice(0, index + 1),
          parent_id: path[index - 1]?.id ?? null, my_access: 'path_only', created_at: null, has_children: true }));
      });
    };
    for (const tree of this.world.trees.values()) for (const node of tree.nodes.values()) {
      if (node.deleted || places.pathTo(node.id).some((step) => step.deleted)) continue;
      const access = places.access(person, node.id);
      const view = access.permissions.includes(`${node.kind}.view`);
      if (!view && !system) continue;
      const path = this.path(person, node.id);
      addAncestors(path.slice(0, -1));
      const base = places.base(person, node);
      rows.set(node.id, inventoryRow({ id: node.id, kind: node.kind, name: path.at(-1)?.name ?? null,
        participating: access.participating,
        parent_id: node.parent_id, path,
        owner: view ? base.owner : null, created_at: view ? base.created_at : null,
        last_activity: view ? [node.activity_at, node.thread?.last_activity_at, base.updated_at].filter((at): at is string => typeof at === 'string').sort().at(-1) ?? null : null,
        my_access: view ? level(access.permissions, node.kind) : 'path_only',
        ...(view && node.thread?.display !== undefined ? { thread_display: node.thread.display } : {}),
        state: view ? node.thread?.state ?? null : null,
        href: view ? webHref({ kind: node.kind, id: node.id }) : null,
        can_share: view && (access.owns || access.permissions.includes('access.manage') || access.permissions.length > 0),
        settings_href: view && access.permissions.includes('settings.manage') ? node.kind === 'org' ? `/org/${node.id}/settings/general` : node.kind === 'project' ? `/project/${node.id}/settings` : null : null,
        tags: view ? base.tags : [],
      }));
    }
    for (const { ref: doc, path: unsafePath } of this.world.documents.forSearch(person)) {
      const path = this.path(person, doc.node.id);
      addAncestors(path);
      const stored = this.world.documents.docs.get(doc.id);
      const editor = human(doc.updated_by);
      rows.set(doc.id, inventoryRow({ id: doc.id, kind: 'document', name: doc.title, parent_id: doc.node.id,
        path: path.length === 0 ? unsafePath : path, owner: doc.owner, created_by: human(doc.created_by),
        created_at: stored?.createdAt ?? MADE_AT, last_activity: doc.updated_at, size_bytes: doc.bytes + doc.files_bytes,
        ...(editor === null ? {} : { activity_by: editor }), document_unread: editor?.id !== person.me.id && (doc.seen_seq === null || doc.seq > doc.seen_seq),
        participating: doc.owner.id === person.me.id, href: webHref({ kind: 'document', id: doc.id }), can_share: true, my_access: doc.owner.id === person.me.id ? 'manage' : 'view' }));
    }
    for (const { chat, label, in: participant } of this.world.chats.forSearch(person)) {
      if (chat.kind !== 'node' && !participant) continue;
      const path = chat.node === null ? [] : this.path(person, chat.node.id);
      addAncestors(path);
      const org = path[0]?.id ?? chat.org_id;
      const parent = chat.node?.id ?? org;
      rows.set(chat.id, inventoryRow({ id: chat.id, kind: 'chat', name: label, parent_id: parent, path,
        participating: (this.world.chats.states.get(`${person.me.username}:${chat.id}`)?.effective_level ?? (participant ? 'all' : 'off')) !== 'off',
        href: webHref({ kind: 'chat', id: chat.id, nodeId: chat.node?.id, orgId: org, path }) }));
    }
    const resources: readonly [EverythingKind, string][] = [['environment', '/environments'], ['provider', '/providers'], ['agent', '/agents'], ['machine', '/machines'], ['host', '/hosts'], ['mount', '/mounts'], ['mcp_server', '/mcp-servers'], ['template', '/templates']];
    for (const [kind, route] of resources) {
      const response = this.world.resources.handle(person, 'GET', route, new URLSearchParams(), {});
      const entries: unknown[] = Array.isArray(response?.body) ? response.body : isRecord(response?.body) && Array.isArray(response.body.items) ? response.body.items : [];
      for (const resource of entries) {
        if (!isRecord(resource) || typeof resource.id !== 'string' || typeof resource.name !== 'string') continue;
        const hidden = resource.content_hidden === true;
        const detail = hidden ? null : this.world.resources.inventoryView(person, kind, resource.id);
        const home = isRecord(resource.home) && typeof resource.home.id === 'string' ? resource.home.id : null;
        const path = home === null ? [] : this.path(person, home);
        const access = detail?.value.access;
        const state = detail?.kind === 'host' ? detail.value.schedulable ? 'open' : 'closed'
          : detail?.kind === 'agent' ? detail.value.disabled ? 'disabled' : 'enabled'
          : detail?.kind === 'template' ? detail.value.enabled ? 'enabled' : 'disabled'
          : detail?.kind === 'machine' ? detail.value.status === 'revoked' ? 'revoked' : detail.value.online ? 'online' : 'offline'
          : typeof resource.status === 'string' ? resource.status : null;
        rows.set(resource.id, inventoryRow({ id: resource.id, kind, name: resource.name, parent_id: 'resources', owner: hidden ? null : human(resource.owner),
          resource: detail, path, admin_access: access?.admin_access,
          participating: !hidden && isRecord(resource.access) && resource.access.participating === true,
          href: hidden ? null : webHref({ kind, id: resource.id }), state: hidden ? null : state,
          created_at: hidden ? null : MADE_AT, my_access: hidden ? 'path_only' : human(resource.owner)?.id === person.me.id ? 'manage' : 'view' }));
      }
    }
    const parents = new Set([...rows.values()].flatMap((row) => row.parent_id === null ? [] : [row.parent_id]));
    for (const row of rows.values()) {
      row.has_children = parents.has(row.id);
      const pathLabel = row.path.filter((step) => step.id !== row.id).map((step) => step.name ?? '…').join(' / ');
      row.place_label = pathLabel === ''
        ? (row.kind === 'org' && row.parent_id === null && row.my_access !== 'path_only' ? places.addressOf(row.id) : null)
        : pathLabel;
    }
    return [...rows.values()];
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams): Answer | null {
    if (method === 'GET' && path === '/resources/summary') {
      const rows = this.rows(person, false).filter((row) => row.parent_id === 'resources');
      const count = (kind: EverythingKind) => rows.filter((row) => row.kind === kind).length;
      return { status: 200, body: { providers: count('provider'), agents: count('agent'), machines: count('machine'),
        environments: count('environment'), templates: count('template'), hosts: count('host'), mcp: count('mcp_server'), mounts: count('mount'), skills: count('skill') } };
    }
    if (method !== 'GET' || path !== '/everything') return null;
    const scope = query.get('scope') ?? 'mine';
    if (!['mine', 'system'].includes(scope)) throw new Refusal(400, 'request.invalid', 'Choose a scope.');
    const system = scope === 'system';
    if (system && !person.me.is_system_admin) throw new Refusal(403, 'authz.denied', 'Only a system administrator may open the whole system.');
    const parent = query.get('parent');
    const recent = query.get('recent');
    if (recent !== null && recent !== 'opened' && recent !== 'active') throw new Refusal(400, 'request.invalid', 'Choose a recent order.');
    const navigation = query.get('navigation') === 'true';
    const typed = (query.get('q') ?? '').trim();
    const words = wordsOf(typed).length === 0 ? '' : typed;
    const cursor = query.get('cursor');
    let continued: Cursor | null = null;
    if (cursor !== null) {
      try { continued = JSON.parse(Buffer.from(cursor, 'base64url').toString()) as Cursor; }
      catch { throw new Refusal(400, 'request.invalid', 'Use the next page of this view.'); }
    }
    const scoreAt = continued?.scoreAt ?? Date.now();
    const all = this.rows(person, system).map((row) => {
      const at = row.last_opened ?? this.world.search.lastVisitAt(person, row);
      return at === undefined ? row : { ...row, last_opened: at };
    });
    const kind = (query.get('kinds') ?? '').split(',').filter(Boolean);
    const from = query.get('created_from');
    const to = query.get('created_to');
    const activeFrom = query.get('active_from');
    const owner = query.get('owner');
    const adminAccess = query.get('admin_access');
    const participating = query.get('participating') === 'true';
    const details = query.get('resource_details') === 'true';
    const state = query.get('resource_state'), harness = query.get('harness'), subkind = query.get('resource_kind'), home = query.get('home');
    const resourceView = query.get('resource_view');
    const inView=(row:EverythingRow)=>{
      if(resourceView===null||row.resource===undefined||row.resource===null)return true;
      const own=row.resource.value.owner?.id===person.me.id;
      const prefix=row.kind==='mcp_server'?'mcp':row.kind;
      const permissions=row.resource.value.access?.permissions??[];
      const manage=own||permissions.some((permission)=>permission===`${prefix}.manage`);
      return resourceView==='mine'?own:resourceView==='managed'?manage:resourceView==='usable'?manage||permissions.some((permission)=>permission===`${prefix}.use`):true;
    };
    if (details && (parent !== 'resources' || kind.length !== 1)) throw new Refusal(400, 'request.invalid', 'Choose one resource list.');
    const providers = resourceView === null ? null : this.world.resources.handle(person, 'GET', '/providers', new URLSearchParams({ scope: resourceView }), {});
    const eligibleProviders = providers === null || !Array.isArray(providers.body) ? null : new Set(providers.body.filter(isRecord).map((row) => row.id));
    const filtering = kind.length > 0 || owner !== null || from !== null || to !== null || activeFrom !== null || adminAccess !== null || participating;
    const matching = all.filter((row) => (row.my_access !== 'path_only' || row.parent_id === 'resources' && owner === null && from === null && to === null && activeFrom === null) &&
      (recent === null || row.my_access !== 'path_only' && (recent === 'opened' ? row.last_opened !== undefined && row.last_opened !== null : row.kind === 'thread' || row.kind === 'document')) &&
      (!navigation || ['org', 'project', 'topic', 'thread', 'document', 'chat'].includes(row.kind) && (kind.length === 0 || kind.includes('thread') || !row.path.some((step) => step.kind === 'thread'))) &&
      (query.get('show_archived') !== 'false' || !row.path.some((step) => this.world.places.rowOf(step.id)?.archived === true)) &&
      (query.get('hide_done') !== 'true' || !row.path.some((step) => this.world.places.rowOf(step.id)?.topic?.status === 'done' && !this.world.places.pathTo(query.get('keep') ?? '').some((part) => part.id === step.id))) &&
      inView(row) &&
      (!details || row.my_access !== 'path_only' && row.resource !== undefined && row.resource !== null) &&
      (state === null || row.state === state) &&
      (harness === null || row.resource?.kind === 'agent' && row.resource.value.harness_kind === harness) &&
      (subkind === null || row.resource?.kind === 'template' && row.resource.value.kind === subkind
        || row.resource?.kind === 'environment' && (row.resource.value.template === null ? 'machine' : 'template') === subkind) &&
      (home === null || row.path.some((step) => step.id === home)) &&
      (eligibleProviders === null || row.kind !== 'provider' || eligibleProviders.has(row.id)) &&
      (adminAccess === null || (row.admin_access !== undefined && row.admin_access !== null) === (adminAccess === 'true')) &&
      (!participating || row.participating === true) &&
      (kind.length === 0 || kind.includes(row.kind)) && (owner === null || row.owner?.id === owner) &&
      (from === null || row.created_at !== null && row.created_at >= from) &&
      (to === null || row.created_at !== null && row.created_at <= to) &&
      (activeFrom === null || row.last_activity !== null && row.last_activity >= activeFrom) &&
      (words === '' || matchesQuery(words, details ? [row.name, row.owner?.display_name, row.owner?.username, ...row.path.map((step) => step.name)].filter((name): name is string => typeof name === 'string').join(' ') : row.name)));
    const matchingIds = new Set(matching.map((row) => row.id));
    const byId = new Map(all.map((row) => [row.id, row]));
    const ancestors = new Set<string>();
    for (const row of matching) {
      let at = row.parent_id;
      const visited = new Set<string>();
      while (at !== null && !visited.has(at)) {
        visited.add(at); ancestors.add(at); at = byId.get(at)?.parent_id ?? null;
      }
    }
    const selectedIds = new Set([...matchingIds, ...ancestors]);
    const selectedParents = new Set(all.flatMap((row) => selectedIds.has(row.id) && row.parent_id !== null ? [row.parent_id] : []));
    let rows = all.filter((row) => {
      if (words !== '' || recent !== null) {
        if (!matchingIds.has(row.id)) return false;
        if (parent === null) return true;
        if (parent === 'resources') return row.parent_id === 'resources';
        if (parent === 'shared') return row.path[0]?.id !== undefined && !this.world.places.isMember(person, row.path[0].id);
        return (recent === null || row.id !== parent) && (row.id === parent || row.parent_id === parent || row.path.some((step) => step.id === parent));
      }
      if (filtering && !selectedIds.has(row.id)) return false;
      if (parent === 'shared') return row.kind === 'org' && row.parent_id === null && !this.world.places.isMember(person, row.id);
      if (parent !== null) return row.parent_id === parent;
      return row.kind === 'org' && row.parent_id === null && (system || this.world.places.isMember(person, row.id));
    }).map((row) => ({ ...row, has_children: recent === null && words === '' && row.has_children && (!filtering || selectedParents.has(row.id)) }));
    if (parent === null && words === '' && recent === null) {
      rows.push(inventoryRow({ id: 'resources', kind: 'group', group: 'resources', name: 'resources', created_at: null, my_access: 'path_only', has_children: all.some((row) => row.parent_id === 'resources' && (!filtering || selectedIds.has(row.id))) }));
      if (!system) rows.push(inventoryRow({ id: 'shared', kind: 'group', group: 'shared', name: 'shared', created_at: null, my_access: 'path_only', has_children: all.some((row) => row.kind === 'org' && row.parent_id === null && !this.world.places.isMember(person, row.id) && (!filtering || selectedIds.has(row.id))) }));
    }
    const sort = query.get('sort') ?? (recent === 'opened' ? 'opened' : recent === 'active' ? 'last_activity' : 'name');
    const desc = (query.get('direction') ?? (recent === null ? 'asc' : 'desc')) === 'desc';
    const sortKey = (row: EverythingRow): string | number | null => {
      if (row.my_access === 'path_only' && row.kind !== 'group') return sort === 'name' ? row.name?.toLowerCase() ?? null : null;
      switch (sort) {
        case 'kind': return row.kind;
        case 'owner': return row.owner?.display_name.toLowerCase() ?? null;
        case 'created_by': return row.created_by?.display_name.toLowerCase() ?? null;
        case 'created': return row.created_at;
        case 'last_activity': return row.last_activity;
        case 'opened': return row.last_opened ?? null;
        case 'where': return row.place_label?.toLowerCase() ?? null;
        case 'activity_by': return row.activity_by?.display_name.toLowerCase() ?? null;
        case 'load': return row.resource?.kind === 'host' && row.resource.value.capacity.memory_mb > 0 ? row.resource.value.declared.memory_mb / row.resource.value.capacity.memory_mb : null;
        case 'use': return row.library?.uses?.all_time ?? null;
        case 'running': return row.resource?.kind === 'host' ? row.resource.value.running.environment + row.resource.value.running.provider : null;
        case 'size': return row.size_bytes === null ? null : String(row.size_bytes).padStart(20, '0');
        case 'state': return row.state;
        case 'my_access': return ({ path_only: '0', view: '1', take_part: '2', manage: '3' } as const)[row.my_access];
        case 'tags': {
          const names = row.tags.map((tag) => tag.name.toLowerCase()).sort().join(',');
          return names === '' ? null : names;
        }
        default: return row.name?.toLowerCase() ?? null;
      }
    };
    const compare = (a: Position, b: Position): number => {
      const byKey = a.key === b.key ? 0 : a.key === null ? 1 : b.key === null ? -1 : (typeof a.key === 'number' && typeof b.key === 'number' ? a.key-b.key : String(a.key).localeCompare(String(b.key))) * (desc ? -1 : 1);
      const byActivity = a.activity === b.activity ? 0 : a.activity === null ? 1 : b.activity === null ? -1 : b.activity.localeCompare(a.activity);
      return b.relevance - a.relevance || b.visitScore - a.visitScore || byActivity || byKey || a.kind.localeCompare(b.kind) || a.id.localeCompare(b.id);
    };
    const position = (row: EverythingRow): Position => ({ relevance: recent !== null ? 0 : matchScore(row.name ?? '', words),
      visitScore: recent !== null || words === '' ? 0 : this.world.search.visitWeight(person, row, scoreAt), activity: recent !== null || words === '' ? null : row.last_activity,
      key: sortKey(row), kind: row.kind, id: row.id });
    rows.sort((a, b) => compare(position(a), position(b)));
    const selected = new URLSearchParams(query); selected.delete('cursor'); selected.delete('limit'); selected.sort();
    const selection = selected.toString();
    if (continued !== null) {
      const after = continued;
      if (after.selection !== selection) throw new Refusal(400, 'request.invalid', 'Use the next page of this same view.');
      rows = rows.filter((row) => compare(position(row), after) > 0);
    }
    const limit = Number(query.get('limit') ?? PAGE_DEFAULT);
    if (!Number.isInteger(limit) || limit < 1 || limit > PAGE_MAX) throw new Refusal(400, 'request.invalid', 'Choose between 1 and 200 rows.');
    const items = rows.slice(0, limit);
    const last = items.at(-1);
    const shown = details ? items : items.map(({ resource: _resource, ...row }) => row);
    return { status: 200, body: { items: shown, next_cursor: rows.length > limit && last !== undefined ? Buffer.from(JSON.stringify({ selection, scoreAt, ...position(last) })).toString('base64url') : null } };
  }

  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path !== '/rows') return null;
    const username = typeof body.username === 'string' ? body.username : 'ada';
    const scope = typeof body.scope === 'string' ? body.scope : 'all';
    this.overrides.set(`${username}:${scope}`, Array.isArray(body.rows) ? body.rows.map((row) => inventoryRow(row as EverythingRow)) : []);
    return { status: 204 };
  }
}
