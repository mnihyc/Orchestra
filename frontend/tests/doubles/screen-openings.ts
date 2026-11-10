/** Screen transports reuse native fixture projections and preserve each caller's admission. */
import type { ActivityEntryPage, ChatOpening, FirstSteps, Node, NodeChatOpening, NodeDefaultsOpening, NodeDocumentPage, NodeOverview, NodeKind, NodeRef, OrgOverview, PortOpeningPage, Problem, ResolvedSetting, UsageAnalytics } from '../../src/generated/api-types.ts';
import { Refusal, type Answer, type Person, type World } from './world.ts';

type NativePart<T> = readonly [T | null, Problem | null, number | null];
function splitPart<T>(reply: Answer): NativePart<T> {
  if (reply.status < 400) return [reply.body as T, null, null];
  const raw = Object.entries(reply.headers ?? {}).find(([key]) => key.toLowerCase() === 'retry-after')?.[1];
  const seconds = raw !== undefined && /^\d+$/.test(raw) ? Number(raw) : null;
  return [null, reply.body as Problem, seconds !== null && Number.isSafeInteger(seconds) ? seconds : null];
}

/** The same native GET and current browser principal own every member and its original refusal. */
async function nativePart<T>(world: World, person: Person, path: string): Promise<NativePart<T>> {
  try { return splitPart<T>(await world.requestFeeds.read(person, path)); }
  catch (error) { if (error instanceof Refusal) return splitPart<T>(error.answer); throw error; }
}

/** One transport, the same native authorized readers and page bounds as the product. */
export async function orgOverview(world: World, person: Person, id: string, query: URLSearchParams): Promise<OrgOverview> {
  const org = (await world.requestFeeds.read(person, `/nodes/${id}`)).body as Node;
  if (org.kind !== 'org') throw new Refusal(404, 'not_found', 'the address is not an organization');
  const children = (await world.requestFeeds.read(person, `/nodes/${id}/children?archived=exclude&limit=50${query.get('participating') === 'true' ? '&participating=true' : ''}`)).body as OrgOverview['children'];
  const recent = (await world.requestFeeds.read(person, `/search/names?kinds=thread&limit=50&org_id=${org.org_id}`)).body as OrgOverview['recent'];
  function section<T>([data, problem, retry_after]: NativePart<T>): { status: 'ready'; data: T } | { status: 'failed'; problem: Problem; retry_after: number | null } {
    if (problem !== null) return { status: 'failed', problem, retry_after };
    if (data === null) throw new Error('The native overview section has no data');
    return { status: 'ready', data };
  }
  const omitted = { status: 'omitted' } as const;
  const nonempty = children.items.length > 0;
  const now = new Date(); const to = now.toISOString().slice(0, 10);
  const monday = new Date(Date.parse(`${to}T00:00:00Z`) - ((now.getUTCDay() + 6) % 7) * 86_400_000).toISOString().slice(0, 10);
  const usage = new URLSearchParams({ scope: `node:${id}`, from: query.get('from') ?? monday, to: query.get('to') ?? to, breakdowns: `person,${query.get('usage_by') ?? 'model'}`, limit: '10' });
  const firstSteps: OrgOverview['first_steps'] = org.org_kind !== 'personal' && !org.archived && org.parent_id === null && org.depth === 0 && org.access.owns ? section(await nativePart<FirstSteps>(world, person, `/me/first-steps?org_id=${id}`)) : omitted;
  return { org, children, recent,
    outside_status: firstSteps.status === 'ready' && firstSteps.data.steps.some((step) => step.id === 'away' && step.state !== 'unknown') ? { status: 'ready', data: world.away.outsideFor(person, id) } : omitted,
    first_steps: firstSteps,
    usage: nonempty ? section(await nativePart<UsageAnalytics>(world, person, `/usage?${usage}`)) : omitted,
    activity: nonempty ? section(await nativePart<ActivityEntryPage>(world, person, `/nodes/${id}/activity?limit=50&depth=summary`)) : omitted,
    outside_openings: nonempty && org.access.administers ? section(await nativePart<PortOpeningPage>(world, person, `/nodes/${id}/port-openings`)) : omitted,
  };
}

/** A project/topic opening uses each section's existing independently admitted native reader. */
export async function nodeOverview(world: World, person: Person, id: string, query: URLSearchParams): Promise<NodeOverview> {
  const node = (await world.requestFeeds.read(person, `/nodes/${id}`)).body as Node;
  if (node.kind !== 'project' && node.kind !== 'topic') throw new Refusal(404, 'not_found', 'the address is not a project or topic');
  const now = new Date(), today = now.toISOString().slice(0, 10);
  const monday = new Date(Date.parse(`${today}T00:00:00Z`) - ((now.getUTCDay() + 6) % 7) * 86_400_000).toISOString().slice(0, 10);
  const usage = new URLSearchParams({ scope: `node:${id}`, from: query.get('from') ?? monday, to: query.get('to') ?? today });
  if (query.has('breakdowns')) usage.set('breakdowns', query.get('breakdowns') ?? '');
  if (query.has('limit')) usage.set('limit', query.get('limit') ?? '');
  const section = <T,>([data, problem, retry_after]: NativePart<T>) => problem === null
    ? { status: 'ready' as const, data: data as T } : { status: 'failed' as const, problem, retry_after };
  return { node, usage: section(await nativePart<UsageAnalytics>(world, person, `/usage?${usage}`)),
    outside_openings: node.access.administers ? section(await nativePart<PortOpeningPage>(world, person, `/nodes/${id}/port-openings`)) : { status: 'omitted' } };
}

/** The same operation's bounded place selector never loads unshown overview sections. */
export async function orgLandingPlaces(world: World, person: Person, root: string, query: URLSearchParams): Promise<Node[]> {
  const context = (await world.requestFeeds.read(person, `/nodes/${root}`)).body as Node;
  if (context.kind !== 'org') throw new Refusal(404, 'not_found', 'the context is not an organization');
  if (['participating', 'from', 'to', 'usage_by'].some((key) => query.has(key))) throw new Refusal(400, 'request.invalid', 'place selection cannot include overview controls');
  const ids = (query.get('resolve_ids') ?? '').split(',');
  if (ids.length > 50 || ids.some((id) => !/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(id))) throw new Refusal(400, 'request.invalid', 'place selection must fit one native page of identifiers');
  const nodes: Node[] = [];
  for (const id of new Set(ids)) {
    try {
      const reply = await world.requestFeeds.read(person, `/nodes/${id}`);
      if ([403, 404, 410].includes(reply.status)) continue;
      if (reply.status !== 200) throw new Error('The native node selector failed');
      nodes.push(reply.body as Node);
    } catch (error) { if (!(error instanceof Refusal) || ![403, 404, 410].includes(error.answer.status)) throw error; }
  }
  return nodes;
}

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
function selectedProvider(rows: ResolvedSetting[] | null, key: string, member?: string): string | null {
  const resolved = rows?.find((row) => row.key === key)?.value;
  const value = member === undefined ? resolved : record(resolved) ? resolved[member] : null;
  return typeof value === 'string' && /^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/i.test(value) ? value : null;
}

async function defaultsOpening(world: World, person: Person, node: Node, query: URLSearchParams): Promise<NodeDefaultsOpening> {
  const [registry, registry_problem, registry_retry_after]: NativePart<NonNullable<NodeDefaultsOpening['registry']>> = query.get('settings_only') === 'true'
    ? [null, null, null] : await nativePart<NonNullable<NodeDefaultsOpening['registry']>>(world, person, '/settings/registry');
  const [settings, settings_problem, settings_retry_after] = await nativePart<NonNullable<NodeDefaultsOpening['settings']>>(world, person, `/nodes/${node.id}/settings`);
  const [secrets, secrets_problem, secrets_retry_after]: NativePart<NonNullable<NodeDefaultsOpening['secrets']>> = query.get('secret_names') === 'true'
    ? await nativePart<NonNullable<NodeDefaultsOpening['secrets']>>(world, person, `/nodes/${node.id}/secrets`) : [null, null, null];
  const wanted: { setting: string; id: string }[] = [];
  if (query.get('default_provider_name') === 'true') {
    const id = selectedProvider(settings, 'agent.default_provider_id');
    if (id !== null) wanted.push({ setting: 'agent.default_provider_id', id });
  }
  if (query.get('embeddings_provider_name') === 'true' && node.kind === 'org') {
    const id = selectedProvider(settings, 'search.semantic.embeddings', 'provider_id');
    if (id !== null) wanted.push({ setting: 'search.semantic.embeddings', id });
  }
  const providers: NodeDefaultsOpening['providers'] = [];
  for (const { setting, id } of wanted) {
    const held = providers.find((entry) => entry.id === id);
    const [provider, problem, retry_after]: NativePart<NonNullable<NodeDefaultsOpening['providers'][number]['provider']>> = held === undefined
      ? await nativePart<NonNullable<NodeDefaultsOpening['providers'][number]['provider']>>(world, person, `/providers/${id}`)
      : [held.provider, held.problem, held.retry_after];
    providers.push({ setting, id, provider, problem, retry_after });
  }
  return { node, registry, registry_problem, registry_retry_after, settings, settings_problem, settings_retry_after,
    secrets, secrets_problem, secrets_retry_after, providers };
}

export async function chatOpening(world: World, person: Person, id: string, query: URLSearchParams): Promise<ChatOpening> {
  if (query.has('since')) throw new Refusal(400, 'request.invalid', 'an opening starts without a position');
  const sync = (await world.requestFeeds.read(person, `/chats/${id}/sync?${query}`)).body as ChatOpening['sync'];
  let draft: ChatOpening['draft'];
  try { draft = (await world.requestFeeds.read(person, `/chats/${id}/draft`)).body as ChatOpening['draft']; }
  catch (error) { if (error instanceof Refusal && error.answer.status === 403) draft = null; else throw error; }
  return { sync, draft };
}

export async function nodeChatOpening(world: World, person: Person, id: string, query: URLSearchParams): Promise<NodeChatOpening> {
  const own = (await world.requestFeeds.read(person, `/nodes/${id}`)).body as Node;
  const wanted: NodeKind[] = own.kind === 'org' || own.kind === 'project' ? [own.kind] : own.kind === 'thread' ? ['thread', 'topic', 'project'] : ['topic', 'project'];
  const scopes: NodeChatOpening['scopes'] = [];
  for (const kind of wanted) {
    const target = [...own.path].reverse().find((node) => node.kind === kind && !node.redacted);
    if (target === undefined) continue;
    try {
      const node = (await world.requestFeeds.read(person, `/nodes/${target.id}`)).body as Node;
      const chat = (await world.requestFeeds.read(person, `/chats/${node.chat_id}`)).body as ChatOpening['sync']['chat'];
      const reference: NodeRef = { id: node.id, kind: node.kind, name: node.name, redacted: false };
      scopes.push({ node: reference, chat: { id: chat.id, seq: chat.seq } });
    } catch (error) { if (!(error instanceof Refusal) || ![403, 404].includes(error.answer.status)) throw error; }
  }
  const chosen = scopes.find((scope) => scope.chat.id === query.get('chat_id'))
    ?? scopes.find((scope) => scope.node.kind === query.get('scope')) ?? scopes[0];
  const paging = new URLSearchParams(); if (query.has('limit')) paging.set('limit', query.get('limit') ?? '');
  const opening = chosen === undefined ? null : await chatOpening(world, person, chosen.chat.id, paging);
  const result = { scopes, opening,
    ...(opening === null ? {} : world.summaries.opening(person, opening.sync.chat)),
    ...(query.get('defaults') === 'true' ? { defaults: await defaultsOpening(world, person, own, query) } : {}) };
  return result;
}

export async function documentPages(world: World, person: Person, id: string, query: URLSearchParams): Promise<NodeDocumentPage[]> {
  const ids = (query.get('node_ids') ?? '').split(',');
  // The same native maximum page size bounds the batch.
  if (ids[0] !== id || ids.length > 200) throw new Refusal(400, 'request.invalid', 'the node batch must fit one page');
  const pages: NodeDocumentPage[] = [];
  for (const nodeId of ids) {
    try {
      await world.requestFeeds.read(person, `/nodes/${nodeId}`);
      const paging = new URLSearchParams(); if (query.has('limit')) paging.set('limit', query.get('limit') ?? '');
      const page = (await world.requestFeeds.read(person, `/nodes/${nodeId}/documents?${paging}`)).body as NonNullable<NodeDocumentPage['page']>;
      pages.push({ node_id: nodeId, page, error: null });
    } catch (error) {
      if (!(error instanceof Refusal)) throw error;
      pages.push({ node_id: nodeId, page: null, error: error.answer.body as Problem });
    }
  }
  return pages;
}
