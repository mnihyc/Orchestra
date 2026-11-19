/** What the fake API says of finding things: the names the finder lists, the search of what was written, and the places a person went to lately.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: a name is listed
 * only for whoever may see the thing (nodes by what `places` says the caller holds, resources by what the
 * lists of `resources` show them, people of the orgs they belong to), recents come first and then a match
 * at the beginning of a name, and a hit of what was written is returned only for a thread the caller may
 * view. Nothing here knows more of the world than the other doubles do: the tree, the people, the
 * resources and the items of each thread are read from them when a request comes.
 *
 * The documents and the chats that a search finds are the ones the documents and the chats hold. What only a test can do (fill the world with names, hold
 * answers back, make the names fail, set the recents) is under `/__double/search`.
 */
import type {
  Item,
  NameHit,
  NameKind,
  NodeRef,
  SearchHit,
  SearchHitPage,
  SearchStreamEvent,
  TreeNode,
  UserRef,
} from '../../src/generated/api-types.ts';
import { ACME, id as fixtureId, MADE_AT, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';
import { textOf } from './chats.ts';
import { webHref } from './destinations.ts';
import { matches as queryMatches, matchScore, wordsOf } from '../../src/features/finder/ranking.ts';

/** The most a page of a list holds, and how many names a request without a limit gets: the contract's numbers. */
const NAME_LIMIT_DEFAULT = 20;
const NAME_LIMIT_MAX = 50;
const PAGE_DEFAULT = 20;
const PAGE_MAX = 200;
/** The least characters of a search of what was written, and of one that can use the index. */
const QUERY_MIN = 2;
const QUERY_INDEXED_MIN = 3;
/** The server retains fifty personal shortcuts and halves habit weight each week. */
const RECENTS_KEPT = 50;
const VISIT_HALF_LIFE_MS = 7 * 24 * 60 * 60 * 1_000;
/** The words around the first match: this many characters before it, and the snippet is at most this long. */
const SNIPPET_BEFORE = 60;
const SNIPPET_MAX = 240;

/** The kinds of resource and the request that lists each of them for a person. */
const RESOURCE_LISTS: readonly [NameKind, string][] = [
  ['provider', '/providers'],
  ['agent', '/agents'],
  ['environment', '/environments'],
  ['host', '/hosts'],
  ['template', '/templates'],
  ['machine', '/machines'],
  ['mcp_server', '/mcp-servers'],
  ['mount', '/mounts'],
];

const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const asText = (value: unknown): string => (typeof value === 'string' ? value : '');

/** Text without case or accents, for the substring match the API promises. */
const fold = (text: string): string => text.normalize('NFD').replace(/\p{M}/gu, '').toLowerCase();

/** Structured content predicates mirror the served filters; authored @text is not a mention. */
function contentHas(value: unknown, predicate: (row: Record<string, unknown>) => boolean): boolean {
  if (Array.isArray(value)) return value.some((item) => contentHas(item,predicate));
  return isRecord(value) && (predicate(value) || Object.values(value).some((item) => contentHas(item,predicate)));
}
const toolImage = (value: unknown): Record<string, unknown> | null => isRecord(value) && isRecord(value.tool) && value.tool.kind === 'image' ? value.tool : null;
const hasFile = (value: unknown): boolean => toolImage(value) !== null || contentHas(value,(row) => ['file','image','attachment'].includes(String(row.type)) || (Array.isArray(row.attachments) && row.attachments.length > 0) || (Array.isArray(row.images) && row.images.length > 0));
const namesPerson = (value: unknown, id: string): boolean => contentHas(value,(row) => row.type === 'mention' && row.user_id === id);
const matchesType = (actual: string, requested: string | null): boolean => requested === null || requested === '*/*'
  || actual.toLowerCase() === requested.toLowerCase()
  || requested.endsWith('/*') && actual.toLowerCase().startsWith(requested.slice(0, -1).toLowerCase());
const hasType = (value: unknown, requested: string | null): boolean => requested === null || requested === '*/*' && hasFile(value)
  || typeof toolImage(value)?.media_type === 'string' && matchesType(String(toolImage(value)?.media_type), requested)
  || contentHas(value, (row) => {
    const file = ['file', 'image', 'attachment'].includes(String(row.type));
    const own = file && typeof row.media_type === 'string' && matchesType(row.media_type, requested);
    const nested = [row.attachments, row.images].some((items) => Array.isArray(items) && items.some((item: unknown) =>
      isRecord(item) && typeof item.media_type === 'string' && matchesType(item.media_type, requested)));
    return own || nested;
  });

export class SearchWorld implements Part {
  readonly world: World;
  /** The nodes each person went to lately, the last first. */
  private recents = new Map<string, string[]>();
  private visits = new Map<string, { count: number; at: number }>();
  /** Names that only a test made, to hold many; every person sees them. */
  private synthetic: NameHit[] = [];
  /** How long a request of this section is held back, and whether the names fail. */
  private delayMs = 0;
  private failing = false;

  constructor(world: World) {
    this.world = world;
  }

  reset(): void {
    this.recents.clear();
    this.visits.clear();
    this.synthetic = [];
    this.delayMs = 0;
    this.failing = false;
  }

  // ---------------------------------------------------------------- names

  /** Notes that somebody went to a node. */
  visited(person: Person, nodeId: string): void {
    const node = this.world.places.rowOf(nodeId);
    if (node !== undefined) this.visitedItem(person, node.kind, nodeId);
  }

  private visitedItem(person: Person, kind: string, id: string): void {
    const key = `${kind}:${id}`;
    const list = (this.recents.get(person.me.username) ?? []).filter((one) => one !== key);
    list.unshift(key);
    const now = Date.now();
    const storageKey = `${person.me.username}:${key}`;
    const old = this.visits.get(storageKey);
    const count = old === undefined ? 1 : old.count * 0.5 ** (Math.max(0, now - old.at) / VISIT_HALF_LIFE_MS) + 1;
    this.visits.set(storageKey, { count, at: now });
    for (const gone of list.slice(RECENTS_KEPT)) this.visits.delete(`${person.me.username}:${gone}`);
    this.recents.set(person.me.username, list.slice(0, RECENTS_KEPT));
  }

  /** The same kind/id visit weight for name and Everything ordering, optionally at a page's pinned clock. */
  visitWeight(person: Person, hit: Pick<NameHit, 'kind' | 'id'>, at = Date.now()): number {
    const held = this.visits.get(`${person.me.username}:${hit.kind}:${hit.id}`);
    return held === undefined ? 0 : held.count * 0.5 ** (Math.max(0, at - held.at) / VISIT_HALF_LIFE_MS);
  }

  /** The reader's visit clock, separately from search's decaying frequency score. */
  lastVisitAt(person: Person, hit: Pick<NameHit, 'kind' | 'id'>): string | undefined {
    const held = this.visits.get(`${person.me.username}:${hit.kind}:${hit.id}`);
    return held === undefined ? undefined : new Date(held.at).toISOString();
  }

  private ref(row: TreeNode): NodeRef {
    return { id: row.id, kind: row.kind, name: row.name, redacted: false, ...(row.thread?.display === undefined ? {} : { thread_display: row.thread.display }) };
  }

  private nodeNames(person: Person): NameHit[] {
    const places = this.world.places;
    const recent = new Set(this.recents.get(person.me.username) ?? []);
    const hits: NameHit[] = [];
    for (const tree of this.world.trees.values()) {
      for (const row of tree.nodes.values()) {
        if (row.deleted) continue;
        if (!places.access(person, row.id).permissions.includes(`${row.kind}.view`)) continue;
        hits.push({
          ...(row.thread?.display === undefined ? {} : { thread_display: row.thread.display }),
          kind: row.kind,
          participating: places.participates(person, row.id),
          state: row.archived ? 'archived' : row.thread?.state ?? row.topic?.status ?? null,
          last_activity: row.thread?.last_activity_at ?? places.metaOf(row.id).updatedAt,
          thread_attention: row.kind === 'thread' ? row.thread?.attention ?? null : null,
          tag: null,
          id: row.id,
          name: row.name,
          color: row.color,
          custom_color: row.custom_color ?? null,
          address: row.kind === 'org' ? this.world.places.addressOf(row.id) : null,
          username: null,
          avatar: null,
          path: places.pathTo(row.id).map((entry) => this.ref(entry)),
          owner: null,
          archived: row.archived,
          recent: recent.has(`${row.kind}:${row.id}`),
        });
      }
    }
    return hits;
  }

  private peopleNames(person: Person): NameHit[] {
    const places = this.world.places;
    // Everybody the caller shares an org with, themselves included.
    const orgs = [...this.world.trees.keys()].filter((root) => places.isMember(person, root) || person.me.is_system_admin);
    const hits: NameHit[] = [];
    const isTeam = (candidate: string): boolean => this.world.orgs.find((org) => org.id === candidate)?.org_kind === 'team';
    for (const other of this.world.people.values()) {
      // Where they are addressed from: the team org they share with the caller, and else their own.
      const root = orgs.find((candidate) => isTeam(candidate) && places.isMember(other, candidate)) ?? orgs.find((candidate) => other.me.personal_org_id === candidate);
      if (root === undefined || other.suspended) continue;
      const row = places.rowOf(root);
      const home = row === undefined ? other.me.username : `${places.metaOf(root).slug ?? row.name.toLowerCase()}.${other.me.username}`;
      hits.push({ kind: 'user', participating: places.participates(person, root), thread_attention:null, id: other.me.id, name: other.me.display_name, tag: null, color: null, custom_color: null, address: home, username: other.me.username, avatar: userRef(other.me).avatar, path: [], owner: null, archived: false, recent: false });
    }
    hits.push({ kind: 'group', thread_attention:null, id: fixtureId(0x32, 1), name: 'Infra team', tag: null, color: null, custom_color: null, address: 'acme.infra-team', username: null, avatar: null, path: [], owner: null, archived: false, recent: false });
    return hits;
  }

  /** What the lists of resources show this person, as names: whatever they may see of providers, agents, environments, hosts, templates and machines. */
  private resourceNames(person: Person): NameHit[] {
    const hits: NameHit[] = [];
    for (const [kind, path] of RESOURCE_LISTS) {
      const answer = this.world.resources.handle(person, 'GET', path, new URLSearchParams(), {});
      if (answer === null || !Array.isArray(answer.body)) continue;
      for (const entry of answer.body as unknown[]) {
        if (!isRecord(entry) || typeof entry.id !== 'string' || typeof entry.name !== 'string') continue;
        hits.push({
          kind,
          participating: isRecord(entry.access) && entry.access.participating === true,
          state: typeof entry.status === 'string' ? entry.status : null,
          last_activity: typeof entry.last_activity_at === 'string' ? entry.last_activity_at : typeof entry.updated_at === 'string' ? entry.updated_at : MADE_AT,
          thread_attention:null,
          id: entry.id,
          tag: null,
          name: entry.name,
          color: null,
          custom_color: null,
          address: typeof entry.address === 'string' ? entry.address : null,
          username: null,
          avatar: null,
          path: [],
          owner: isRecord(entry.owner) ? (entry.owner as unknown as UserRef) : null,
          archived: false,
          recent: false,
        });
      }
    }
    return hits;
  }

  /** The documents the person may read, as names: by the place they are written beside. */
  private documentNames(person: Person): NameHit[] {
    return this.world.documents.forSearch(person).map(
      ({ ref, path }): NameHit => ({
        kind: 'document',
        participating: ref.owner.id === person.me.id || this.world.access.namedOn(person, 'documents', ref.id),
        thread_attention:null,
        tag: null,
        id: ref.id,
        name: ref.title,
        color: null,
        custom_color: null,
        address: null,
        username: null,
        avatar: null,
        path: path.map((entry) => ({ ...entry })),
        owner: ref.owner,
        archived: ref.archived,
        recent: false,
      }),
    );
  }

  /** The chats the person may read, as names: a place's chat by the place, a group or a direct chat by who is in it. */
  private chatNames(person: Person): NameHit[] {
    return this.world.chats.forSearch(person).filter((one) => one.chat.kind === 'node' ? one.messages.some((message) => message.kind === 'message' && !message.deleted) : one.in).map(({ chat, label, path, in: member }): NameHit => ({
      kind: 'chat',
      href: webHref({ kind: 'chat', id: chat.id, nodeId: chat.node?.id, orgId: chat.org_id, path }),
      participating: this.chatParticipation(person, chat.id, chat.node?.id ?? null, member),
      thread_attention:null,
      tag: null,
      id: chat.id,
      name: label,
      color: null,
      custom_color: null,
      address: null,
      username: null,
      avatar: null,
      path,
      owner: null,
      archived: false,
      recent: false,
    }));
  }

  private chatParticipation(person: Person, chat: string, node: string | null, member: boolean): boolean {
    const level = this.world.chats.states.get(`${person.me.username}:${chat}`)?.effective_level ?? (member ? 'all' : 'off');
    return level !== 'off' || node !== null && this.world.places.participates(person, node);
  }

  /** Every name this person may see, of every kind. */
  private allNames(person: Person): NameHit[] {
    const tags = new Map<string, NameHit>();
    for (const tree of this.world.trees.values()) for (const scope of tree.nodes.values()) {
      if (scope.deleted || scope.kind === 'thread' || !this.world.places.visibleTo(person, scope)) continue;
      for (const tag of this.world.places.tagsFor(person, scope.id)) {
        const defining = this.world.places.rowOf(tag.node.id);
        const safeTag = { ...tag, node: defining !== undefined && this.world.places.visibleTo(person,defining)
          ? tag.node : { ...tag.node,name:null,redacted:true } };
        tags.set(tag.id, { kind:'tag',thread_attention:null,id:tag.id,name:tag.name,tag:safeTag,color:tag.color,
          participating:this.world.places.participates(person,tag.node.id),
          custom_color:null, address:null,username:null,avatar:null,owner:null,archived:scope.archived,recent:false,
          path:this.world.places.pathTo(tag.node.id).map((node) => this.world.places.visibleTo(person,node)
            ? this.ref(node) : { id:node.id,kind:node.kind,name:null,redacted:true }) });
      }
    }
    return [
      ...this.nodeNames(person),
      ...this.documentNames(person),
      ...this.chatNames(person),
      ...tags.values(),
      ...this.peopleNames(person),
      ...this.resourceNames(person),
      ...this.synthetic,
    ].map((hit) => ({ ...hit, href: hit.href ?? webHref({ kind: hit.kind, id: hit.id,
      nodeId: hit.path.at(-1)?.id, orgId: hit.path[0]?.id, path: hit.path, username: hit.username }) }));
  }

  names(person: Person, query: URLSearchParams): NameHit[] {
    const typed = fold(query.get('q') ?? '').trim();
    const kindsParam = query.get('kinds');
    const kinds = new Set((kindsParam ?? '').split(',').filter((kind) => kind !== ''));
    const limit = Math.min(NAME_LIMIT_MAX, Math.max(1, Number(query.get('limit') ?? NAME_LIMIT_DEFAULT) || NAME_LIMIT_DEFAULT));
    const root = query.get('org_id');
    const recent = new Set(this.recents.get(person.me.username) ?? []);
    const order = new Map([...recent].map((id, index) => [id, index]));
    const saved = this.world.places.favorites.get(person.me.username) ?? [];
    const savedOrder = new Map(saved.map((key, index) => [key, index]));
    const key = (hit: NameHit) => `${hit.kind}:${hit.id}`;

    let pool = this.allNames(person).filter((hit) => kindsParam === null || kinds.has(hit.kind))
      .map((hit) => ({ ...hit, last_activity: hit.last_activity ?? this.updatedOf(hit), recent: recent.has(key(hit)), starred: savedOrder.has(key(hit)) }));
    const onlyStarred = query.get('starred') === 'true';
    if (onlyStarred) pool = pool.filter((hit) => hit.starred);
    if (query.get('participating') === 'true') pool = pool.filter((hit) => hit.participating === true);
    if (root !== null) pool = pool.filter((hit) => hit.path[0]?.id === root || hit.id === root);

    if (typed === '') {
      if (onlyStarred) return pool.sort((one, other) => (savedOrder.get(key(one)) ?? 0) - (savedOrder.get(key(other)) ?? 0)).slice(0, limit);
      // No letters: the places the caller went to lately, the last first.
      return pool
        .filter((hit) => recent.has(key(hit)))
        .sort((one, other) => (order.get(key(one)) ?? 0) - (order.get(key(other)) ?? 0))
        .slice(0, limit);
    }
    const matches = (hit: NameHit): boolean => queryMatches(typed, hit.name, hit.address, hit.kind === 'user' ? this.usernameOf(hit) : '');
    return pool
      .filter(matches)
      .sort((one, other) => {
        const relevance = matchScore(other.name, typed) - matchScore(one.name, typed);
        if (relevance !== 0) return relevance;
        const recency = this.visitWeight(person, other) - this.visitWeight(person, one);
        if (recency !== 0) return recency;
        return this.updatedOf(other).localeCompare(this.updatedOf(one));
      })
      .slice(0, limit);
  }

  private usernameOf(hit: NameHit): string {
    for (const person of this.world.people.values()) if (person.me.id === hit.id) return `${person.me.username} ${person.me.email ?? ''}`;
    return '';
  }

  private updatedOf(hit: NameHit): string {
    return hit.last_activity ?? (hit.kind === 'org' || hit.kind === 'project' || hit.kind === 'topic' || hit.kind === 'thread' ? this.world.places.metaOf(hit.id).updatedAt : MADE_AT);
  }

  // ---------------------------------------------------------------- what was written

  /** The words an item can be found by. */
  private textOf(item: Item): string {
    const payload = item.payload as unknown as Record<string, unknown>;
    switch (item.kind) {
      case 'user_message':
        return (Array.isArray(payload.parts) ? payload.parts : []).map((part: unknown) => (isRecord(part) ? asText(part.text) : '')).join('\n');
      case 'agent_message':
        return asText(payload.text);
      case 'reasoning':
        return asText(payload.summary);
      case 'tool_call': {
        const tool = isRecord(payload.tool) ? payload.tool : {};
        return `${asText(payload.title)}\n${asText(tool.command)}`;
      }
      case 'error':
        return asText(payload.message);
      case 'announcement':
        return `${asText(payload.title)}\n${asText(payload.body)}`;
      default:
        return '';
    }
  }

  /** The words around the first match, and where in them the matches lie, counted in characters as the API counts. */
  private snippet(text: string, typed: string, regex = false): { snippet: string; matches: [number, number][] } | null {
    const characters = Array.from(text);
    if(regex) {
      let expression:RegExp;
      try {expression=new RegExp(typed,'giu');} catch {throw new Refusal(400,'request.invalid','the expression is invalid');}
      const first=expression.exec(text);
      if(first===null)return null;
      const start=Math.max(0,Array.from(text.slice(0,first.index)).length-SNIPPET_BEFORE);
      const snippet=characters.slice(start,start+SNIPPET_MAX).join('');
      const matches:[number,number][]=Array.from(snippet.matchAll(new RegExp(typed,'giu')),(match)=>{
        const from=Array.from(snippet.slice(0,match.index)).length;
        return [from,from+Array.from(match[0]).length];
      });
      return {snippet,matches};
    }
    const folded = Array.from(fold(text));
    const needles = wordsOf(typed).map((term) => Array.from(term));
    const firsts = needles.map((needle) => indexOf(folded, needle, 0));
    if (firsts.length === 0 || firsts.some((at) => at === -1)) return null;
    const first = Math.min(...firsts);
    const start = Math.max(0, first - SNIPPET_BEFORE);
    const end = Math.min(characters.length, start + SNIPPET_MAX);
    const snippet = characters.slice(start, end).join('');
    const matches: [number, number][] = [];
    for (const needle of needles) {
      for (let at = indexOf(folded, needle, start); at !== -1 && at + needle.length <= end; at = indexOf(folded, needle, at + needle.length)) {
        matches.push([at - start, at - start + needle.length]);
      }
    }
    matches.sort((one, other) => one[0] - other[0] || one[1] - other[1]);
    return { snippet, matches };
  }

  private inScope(nodeId: string, scope: string | null): boolean {
    if (scope === null) return true;
    for (const entry of this.world.places.pathTo(nodeId)) if (entry.id === scope) return true;
    return false;
  }

  search(person: Person, query: URLSearchParams, streamed = false): Answer {
    const raw = query.get('q') ?? '';
    const regex=query.get('regex')==='true';
    const typed = regex ? raw : fold(raw).trim();
    const length = Array.from(typed).length;
    const scope = query.get('scope');
    const places = this.world.places;
    const scopeRow = scope === null ? undefined : places.rowOf(scope);
    const chatId = query.get('chat_id');
    const documentId = query.get('document_id');
    if (chatId !== null && documentId !== null) throw new Refusal(400, 'request.invalid', 'choose one chat or document');
    const chats = this.world.chats.forSearch(person);
    const documents = this.world.documents.forSearch(person);
    const narrow = (scopeRow !== undefined && (scopeRow.kind === 'topic' || scopeRow.kind === 'thread') && places.visibleTo(person, scopeRow))
      || (chatId !== null && chats.some(({ chat, path }) => chat.id === chatId && (scope === null || path.some((node) => node.id === scope) || chat.org_id === scope)))
      || (documentId !== null && documents.some(({ ref }) => ref.id === documentId && this.inScope(ref.node.id, scope)));
    if (streamed ? length<1 : length < QUERY_MIN || (length < QUERY_INDEXED_MIN && !narrow)) {
      throw new Refusal(400, 'request.invalid', 'the words are too short', {
        errors: [{ path: 'q', message: length < QUERY_MIN ? `at least ${QUERY_MIN} characters` : `at least ${QUERY_INDEXED_MIN} characters, unless the search is kept to a topic or a thread` }],
      });
    }
    const kindsParam = query.get('kinds');
    const kinds = new Set((kindsParam ?? '').split(',').filter((kind) => kind !== ''));
    const wants = (kind: string): boolean => kindsParam === null || kinds.has(kind);
    const author = query.get('author_id');
    const creator = query.get('created_by_id');
    const editor = query.get('edited_by_id');
    const owner = query.get('owner_id');
    const dateField = query.get('date_field');
    const titleOnly = query.get('title_only') === 'true';
    const shared = query.get('shared_with_me') === 'true';
    const fileType = query.get('file_type');
    const fileOnly = query.get('with_file') === 'true';
    const mentionOnly = query.get('mentions_me') === 'true';
    const from = query.get('from');
    const to = query.get('to');
    const inSpan = (at: string): boolean => (from === null || at >= from) && (to === null || at <= to);
    const sourceTime = (part: string, created: string, edited: string): boolean => inSpan(dateField === 'created' ? created : dateField === 'edited' ? edited : part);

    const relevance = query.get('sort') === 'relevance';
    const found: (SearchHit & { sortAt: string; relevance: number })[] = [];

    if ((relevance || titleOnly) && (kindsParam === null || titleOnly && [...kinds].some((kind) => kind !== 'document' && kind !== 'chat_message')) && author === null && !fileOnly && fileType === null && !mentionOnly && chatId === null && documentId === null) {
      for (const hit of this.nodeNames(person)) {
        if (!this.inScope(hit.id, scope) || query.get('participating') === 'true' && hit.participating !== true) continue;
        if (kinds.size > 0 && hit.kind !== 'thread') continue;
        const meta = places.metaOf(hit.id);
        if (owner !== null && this.world.people.get(meta.owner)?.me.id !== owner || creator !== null || editor !== null) continue;
        if (shared && !this.world.access.namedAt(person, hit.id)) continue;
        const at = hit.last_activity ?? places.metaOf(hit.id).updatedAt;
        const excerpt = this.snippet(hit.name, typed,regex);
        const node = places.rowOf(hit.id);
        if (excerpt === null || node === undefined || !sourceTime(at, meta.createdAt, meta.updatedAt)) continue;
        const href = webHref({ kind: hit.kind, id: hit.id });
        if (href !== null) found.push({ kind: 'name', node: this.ref(node), path: hit.path, href, at,
          ...excerpt, sortAt: at, relevance: 8 + matchScore(hit.name, typed) });
      }
    }

    for (const row of [...this.world.trees.values()].flatMap((tree) => [...tree.nodes.values()])) {
      if (titleOnly) continue;
      if (row.kind !== 'thread' || row.deleted || chatId !== null || documentId !== null || !this.inScope(row.id, scope)) continue;
      if (!places.access(person, row.id).permissions.includes('thread.view')) continue;
      if (query.get('participating') === 'true' && !places.participates(person, row.id)) continue;
      const path = places.pathTo(row.id);
      const topic = path.at(-2);
      // The project a topic lies in is the one it lies in directly; a folder is a project.
      const project = path.at(-3);
      if (topic === undefined || project === undefined) continue;
      const items = (this.world.threads.snapshot(row.id) as { items: Item[] }).items;
      const meta = places.metaOf(row.id);
      if (owner !== null && this.world.people.get(meta.owner)?.me.id !== owner || creator !== null || editor !== null) continue;
      if (shared && !this.world.access.namedAt(person, row.id)) continue;
      for (const item of items) {
        if ((fileOnly && !hasFile(item.payload)) || !hasType(item.payload, fileType) || (mentionOnly && !namesPerson(item.payload,person.me.id))) continue;
        if (!wants(item.kind) || (author !== null && item.author?.id !== author) || !sourceTime(item.created_at, meta.createdAt, meta.updatedAt)) continue;
        const hit = this.snippet(this.textOf(item), typed,regex);
        if (hit === null) continue;
        found.push({ kind: 'item', item, thread: this.ref(row), topic: this.ref(topic), project: this.ref(project), ...hit, sortAt: item.created_at, relevance: 0 });
      }
    }

    if (wants('chat_message') && documentId === null && !titleOnly && owner === null) {
      for (const { chat, path, messages, in: member } of chats) {
        const rec = this.world.chats.records.get(chat.id);
        if (rec === undefined || creator !== null && (rec.createdBy === null || this.world.people.get(rec.createdBy)?.me.id !== creator)) continue;
        if (shared && (chat.node === null || !this.world.access.namedAt(person, chat.node.id))) continue;
        if (query.get('participating') === 'true' && !this.chatParticipation(person, chat.id, chat.node?.id ?? null, member)) continue;
        if (chatId !== null && chat.id !== chatId) continue;
        const node = path.at(-1);
        if (node !== undefined && !this.inScope(node.id, scope)) continue;
        if (node === undefined && scope !== null && chat.org_id !== scope) continue;
        for (const message of messages) {
          if ((fileOnly && !hasFile(message.parts)) || !hasType(message.parts, fileType) || (mentionOnly && !message.mentions.some((user) => user.id === person.me.id))) continue;
          if (editor !== null && (message.author === null || !('id' in message.author) || message.author.id !== editor)) continue;
          if (!sourceTime(message.created_at, message.created_at, message.edited_at ?? message.created_at) || (author !== null && message.author !== null && 'id' in message.author && message.author.id !== author)) continue;
          const hit = this.snippet(textOf(message.parts), typed,regex);
          if (hit === null) continue;
          found.push({ kind: 'chat_message', message, chat, path, ...hit, sortAt: message.created_at, relevance: 0 });
        }
      }
    }

    if (wants('document') && chatId === null) {
      for (const { ref, path, blocks } of documents) {
        if (query.get('participating') === 'true' && ref.owner.id !== person.me.id && !this.world.access.namedOn(person, 'documents', ref.id)) continue;
        if (documentId !== null && ref.id !== documentId) continue;
        if (!this.inScope(ref.node.id, scope)) continue;
        const stored = this.world.documents.docs.get(ref.id);
        if (stored === undefined) continue;
        if (owner !== null && ref.owner.id !== owner || creator !== null && (!('id' in ref.created_by) || ref.created_by.id !== creator)
          || editor !== null && (ref.updated_by === null || !('id' in ref.updated_by) || ref.updated_by.id !== editor)) continue;
        if (shared && ref.owner.id !== person.me.id && !this.world.access.namedOn(person, 'documents', ref.id)) continue;
        if (fileType !== null && !stored.files.some((candidate) => matchesType(candidate.media_type, fileType))) continue;
        const live = [...stored.blocks.values()].filter((block) => !block.removed);
        if ((fileOnly && stored.files.length === 0) || (mentionOnly && !live.some((block) => namesPerson(block.content,person.me.id)))) continue;
        const by = ref.updated_by ?? ref.created_by;
        const title = this.snippet(ref.title, typed,regex);
        const file = stored.files.find((candidate) => matchesType(candidate.media_type, fileType) && this.snippet(candidate.name, typed,regex) !== null);
        const represented = file ?? (fileOnly || fileType !== null ? stored.files.find((candidate) => matchesType(candidate.media_type, fileType)) : undefined);
        if (title !== null && sourceTime(ref.updated_at, stored.createdAt, stored.updatedAt) && (author === null || ('id' in by && by.id === author))) found.push({ kind: 'document', document: ref, block_id: null, by, path, ...(represented === undefined ? {} : { file: represented }), ...title, sortAt: ref.updated_at, relevance: relevance ? 8 + matchScore(ref.title, typed) : 0 });
        else if (!titleOnly && file !== undefined && sourceTime(file.added_at, stored.createdAt, stored.updatedAt) && (author === null || file.added_by !== null && 'id' in file.added_by && file.added_by.id === author)) {
          const match = this.snippet(file.name, typed,regex);
          if (match !== null) found.push({ kind: 'document', document: ref, block_id: null, by: file.added_by, file, path, ...match, sortAt: file.added_at, relevance: 0 });
        }
        if (titleOnly) continue;
        for (const block of blocks) {
          if (mentionOnly && !live.some((row) => row.id === block.id && namesPerson(row.content,person.me.id))) continue;
          if (!sourceTime(block.at, stored.createdAt, stored.updatedAt) || (author !== null && (block.by === null || !('id' in block.by) || block.by.id !== author))) continue;
          const hit = this.snippet(block.text, typed,regex);
          if (hit !== null) found.push({ kind: 'document', document: ref, block_id: block.id, by: block.by, path, ...(represented === undefined ? {} : { file: represented }), ...hit, sortAt: block.at, relevance: 0 });
        }
      }
    }

    const scoreAt = Date.now();
    const visits = (hit: SearchHit): number => {
      if (!relevance) return 0;
      switch (hit.kind) {
        case 'name': return this.visitWeight(person, { kind: hit.node.kind, id: hit.node.id }, scoreAt);
        case 'item': return this.visitWeight(person, { kind: 'thread', id: hit.thread.id }, scoreAt);
        case 'chat_message': return this.visitWeight(person, { kind: 'chat', id: hit.chat.id }, scoreAt);
        case 'document': return this.visitWeight(person, { kind: 'document', id: hit.document.id }, scoreAt);
      }
    };
    // Shared name tiers, personal visits and activity; chronological mode retains its old tie order.
    found.sort((one, other) => other.relevance - one.relevance || visits(other) - visits(one) || other.sortAt.localeCompare(one.sortAt));
    const limit = Math.min(PAGE_MAX, Math.max(1, Number(query.get('limit') ?? PAGE_DEFAULT) || PAGE_DEFAULT));
    const offset = Math.max(0, Number(query.get('cursor') ?? 0) || 0);
    const page = found.slice(offset, offset + limit).map(({ sortAt: _at, relevance: _relevance, ...hit }): SearchHit => hit);
    return { status: 200, body: { items: page, next_cursor: offset + limit < found.length ? String(offset + limit) : null } };
  }

  // ---------------------------------------------------------------- the requests

  /** Answers the names, the search and the recents, or `null` when the request is not one of them. */
  async handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Promise<Answer | null> {
    const route = `${method} ${path}`;
    const readable = (kind: string, id: string): boolean => {
      if (this.allNames(person).some((hit) => hit.kind === kind && hit.id === id)) return true;
      if (kind !== 'chat') return false;
      try { return this.world.chats.handle(person, 'GET', `/chats/${id}`, new URLSearchParams(), {})?.status === 200; }
      catch { return false; }
    };
    if (route === 'POST /me/visits') {
      if (typeof body.kind !== 'string' || typeof body.id !== 'string' || !readable(body.kind, body.id)) throw new Refusal(404, 'not_found', 'That thing is not available.');
      this.visitedItem(person, body.kind, body.id);
      this.world.users.tell(person.me.username, { what: 'recents' });
      return { status: 204 };
    }
    const favorite = /^(PUT|DELETE) \/me\/favorites\/([^/]+)\/([^/]+)$/.exec(route);
    if (favorite !== null) {
      const kind = favorite[2] ?? '';
      const id = favorite[3] ?? '';
      const key = `${kind}:${id}`;
      const without = (this.world.places.favorites.get(person.me.username) ?? []).filter((one) => one !== key);
      if (method === 'PUT') {
        if (!readable(kind, id)) throw new Refusal(404, 'not_found', 'That thing is not available.');
        if (body.after === null) without.unshift(key);
        else if (body.after === undefined) without.push(key);
        else {
          if (!isRecord(body.after)) throw new Refusal(422, 'request.invalid', 'Choose another favorite.');
          const at = without.indexOf(`${asText(body.after.kind)}:${asText(body.after.id)}`);
          if (at < 0) throw new Refusal(422, 'request.invalid', 'Choose another favorite.');
          without.splice(at + 1, 0, key);
        }
      }
      this.world.places.favorites.set(person.me.username, without);
      this.world.users.tell(person.me.username, { what: 'favorites' });
      return { status: 204 };
    }
    if (route === 'POST /me/recents') {
      if (typeof body.node_id === 'string') this.visited(person, body.node_id);
      return { status: 204 };
    }
    if (route !== 'GET /search/names' && route !== 'GET /search' && route !== 'GET /search/stream') return null;
    if (this.delayMs > 0) await new Promise((resolve) => setTimeout(resolve, this.delayMs));
    if (route === 'GET /search') return this.search(person, query);
    if(route==='GET /search/stream') {
      const selection=new URLSearchParams(query);selection.set('limit',String(PAGE_MAX));
      const page=this.search(person,selection,true).body as SearchHitPage;
      const key=(hit:SearchHit)=>hit.kind==='name'?`name:${hit.node.id}`:hit.kind==='item'?`item:${hit.item.id}`:hit.kind==='chat_message'?`chat:${hit.message.id}`:`doc:${hit.document.id}:${hit.block_id ?? 'title'}`;
      const events:SearchStreamEvent[]=[{type:'options',page_size:PAGE_DEFAULT,reorder_window_secs:5,meaning_pause_ms:800},
        {type:'source',source:'words',state:'running',reason:null,received:0}];
      for(const hit of page.items)events.push({type:'hit',id:key(hit),hit,found_by:'words'});
      events.push({type:'order',received:page.items.length,items:page.items.map((hit,index)=>({id:key(hit),rank:1/(61+index),found_by:'words'}))},
        {type:'source',source:'words',state:'done',reason:null,received:page.items.length},
        {type:'source',source:'meaning',state:'done',reason:query.get('title_only')==='true'?'title_only':query.get('sort')!=='relevance'?'newest':query.get('regex')==='true'?'regex':(query.get('q')??'').includes('"')?'phrase':null,received:0},
        {type:'cursor',cursor:page.next_cursor});
      return {status:200,bytes:Buffer.from(events.map((event)=>JSON.stringify(event)).join('\n')+'\n'),headers:{'Content-Type':'application/x-ndjson'}};
    }
    if (this.failing) throw new Refusal(500, 'internal', 'the names are not available');
    const text = query.get('q')?.trim() ?? '';
    if (text !== '' && Array.from(text).length < QUERY_MIN) throw new Refusal(422, 'request.invalid', 'Type at least two characters.');
    const items = this.names(person, query);
    const limit = Math.min(NAME_LIMIT_MAX, Math.max(1, Number(query.get('limit') ?? NAME_LIMIT_DEFAULT)));
    return { status: 200, body: { items, partial: items.length === limit } };
  }

  /** What only a test can do. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/names': {
        // Fills the world with names: topics, threads and documents of Web, numbered, for a finder that has many to look through.
        const count = typeof body.count === 'number' ? body.count : 0;
        const web = this.world.places.rowOf(ACME.web);
        const acme = this.world.places.rowOf(ACME.org);
        if (web === undefined || acme === undefined) return { status: 409, body: { detail: 'the team is not here' } };
        const kinds: NameKind[] = ['topic', 'thread', 'document'];
        this.synthetic = Array.from({ length: count }, (_, index): NameHit => {
          const kind = kinds[index % kinds.length] ?? 'topic';
          return {
            kind,
            state: kind === 'thread' ? 'idle' : kind === 'topic' ? 'open' : null,
            last_activity: MADE_AT,
            thread_attention:null,
            id: fixtureId(0x3f, index + 1),
            tag: null,
            name: `${kind === 'topic' ? 'Quarterly' : kind === 'thread' ? 'Nightly' : 'Notes on'} report ${String(index + 1).padStart(5, '0')}`,
            color: null,
            custom_color: null,
            address: null,
            username: null,
            avatar: null,
            path: [this.ref(acme), this.ref(web)],
            owner: null,
            archived: false,
            recent: false,
          };
        });
        return { status: 200, body: { count: this.synthetic.length } };
      }
      case '/latency':
        this.delayMs = typeof body.ms === 'number' ? body.ms : 0;
        return { status: 204 };
      case '/fail':
        this.failing = body.names === true;
        return { status: 204 };
      case '/recents': {
        const username = typeof body.username === 'string' ? body.username : '';
        this.recents.set(username, []);
        const person = this.world.people.get(username);
        if (person !== undefined && Array.isArray(body.ids)) {
          const ids = body.ids.filter((id: unknown): id is string => typeof id === 'string');
          for (const id of ids.reverse()) this.visited(person, id);
        }
        return { status: 204 };
      }
      default:
        return null;
    }
  }
}

/** Where `needle` first appears in `haystack` from `from`, both as lists of characters, or -1. */
function indexOf(haystack: readonly string[], needle: readonly string[], from: number): number {
  if (needle.length === 0) return -1;
  for (let at = from; at + needle.length <= haystack.length; at += 1) {
    let same = true;
    for (let offset = 0; offset < needle.length; offset += 1) {
      if (haystack[at + offset] !== needle[offset]) {
        same = false;
        break;
      }
    }
    if (same) return at;
  }
  return -1;
}
