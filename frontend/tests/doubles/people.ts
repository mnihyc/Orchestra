/** Current reader choices, discussion delivery and deliberate read fences in the one fake world. */
import { noticeKind } from './notifications.ts';
import { createHash, randomUUID } from 'node:crypto';
import type { AgentShare, Chat, ChatAgentAskPreview, ChatMentionName, ChatMessage, ChatPart, ContentPart, DocumentDiscussion, Holder, PermissionPage, ResolvedSetting, SettingDef, UserRef, DocumentDiscussionLevel as DiscussionLevel, MentionChoice, Chat as PeopleChat, ChatState as RevisedChatState, ChatMentionTarget as MentionTarget } from '../../src/generated/api-types.ts';
import { MADE_AT, userRef } from './fixtures.ts';
import { message as messageRow } from './chat-rows.ts';
import { copiesOfMessages } from './chats.ts';
import { PeopleImports } from './people-import.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** The chosen rolling successful-post bound, shared across a scripted origin's chats. */
const AGENT_POSTS_PER_HOUR = 60;
const HOUR_MS = 60 * 60 * 1000;
/** The line breaks not allowed in a one-line notification note. */
const LINE_BREAKS = new Set(['\n', '\r', '\u000b', '\u000c', '\u0085', '\u2028', '\u2029']);
const isPersonNotice = (kind: string) => kind === 'person.notify';
/** The chat double's group capacity. */
const GROUP_MAX_MEMBERS = 12;
/** The chosen personal recent-reaction row size. */
const RECENT_CHOICES = 8;

export class PeopleWorld implements Part {
  private readonly world: World;
  levels = new Map<string, DiscussionLevel>();
  private discussions = new Map<string, Answer>();
  groupPolicy: 'writers' | 'managers' = 'writers';
  inspection: Holder[] | null = null;
  followersOverride: number | null = null;
  agents = new Map<string, NonNullable<PeopleChat['agents']>>();
  requests: { method: string; path: string; body: unknown }[] = [];
  imports: PeopleImports;
  private reactions = new Map<string, string[]>();
  private documentPolicy = 'write';
  private templates: string[] = [];
  private priority = new Map<string, boolean>();
  private policyRows = new Map<string, { value: unknown; revision: number; locked: boolean }>();
  private sent = new Map<string, Answer>();
  private personActs = new Map<string, { path: string; body: string }>();
  private agentPosts = new Map<string, number[]>();
  private localStops = new Map<string, Set<string>>();
  constructor(world: World) { this.world = world; this.imports = new PeopleImports(world); }
  raw(...args: Parameters<PeopleImports['raw']>): ReturnType<PeopleImports['raw']> { return this.imports.raw(...args); }
  private names(person: Person, chat: Chat, parts: readonly ChatPart[]): ChatMentionName[] {
    const choices = this.choices(person, chat, '');
    return parts.flatMap((part) => part.type === 'mention' ? [{ target:part.target,
      name:choices.find((choice) => JSON.stringify(choice.target) === JSON.stringify(part.target))?.name ?? 'a private target' }] : []);
  }
  private shown(parts: readonly ChatPart[], names: readonly ChatMentionName[]): ChatPart[] {
    const out: ChatPart[] = []; let joined = false;
    for (const part of parts) {
      const mention = part.type === 'mention';
      const next: ChatPart = mention ? {type:'text',text:`@${names.find((name) => JSON.stringify(name.target) === JSON.stringify(part.target))?.name ?? 'a private target'}`} : part;
      const previous = out.at(-1);
      if ((mention || joined) && next.type === 'text' && previous?.type === 'text') previous.text += next.text;
      else out.push(next);
      joined = mention;
    }
    return out;
  }
  private audience(person: Person, chat: Chat, value: string, targets: MentionTarget[]): UserRef[] {
    const choices = this.choices(person, chat, '');
    const readers = choices.filter((choice) => choice.target.kind === 'user');
    const named = new Set<string>();
    for (const choice of readers) if (value.toLowerCase().includes(`@${choice.name.toLowerCase()}`)) named.add((choice.target as { id: string }).id);
    if (/@all\b/i.test(value)) for (const reader of readers) {
      const state = this.world.chats.states.get(`${reader.name}:${chat.id}`);
      if (state?.effective_level !== undefined && state.effective_level !== 'off') named.add((reader.target as { id: string }).id);
    }
    for (const target of targets) {
      const choice = choices.find((one) => JSON.stringify(one.target) === JSON.stringify(target));
      if (target.kind === 'agent') {
        if (choice === undefined) throw new Refusal(400,'request.invalid','This agent is not currently offered in this chat');
        continue;
      }
      if (!choice?.available) throw new Refusal(403, 'authz.denied', choice?.reason ?? 'This mention is unavailable', { required: 'chat.group_mentions', missing: [] });
      if (target.kind === 'user') named.add(target.id);
      if (target.kind === 'role') for (const reader of readers) {
        const found = this.world.people.get(reader.name);
        if (found !== undefined && this.world.access.isHolder(found, { kind:'role', role:target.role, at:{id:target.node_id,kind:'org',name:null,redacted:true}, count:0 }, target.node_id)) named.add(found.me.id);
      }
      if (target.kind === 'group') {
        const groups = this.world.access.handle(person, 'GET', `/orgs/${chat.org_id}/groups`, new URLSearchParams(), {})?.body as { id:string; name:string; description:string; org_id:string; member_count:number }[] | undefined;
        const group = groups?.find((group) => group.id === target.id);
        if (group !== undefined) for (const reader of readers) {
          const found = this.world.people.get(reader.name);
          if (found !== undefined && this.world.access.isHolder(found, { kind:'group', group }, chat.node?.id ?? chat.org_id)) named.add(found.me.id);
        }
      }
    }
    named.delete(person.me.id);
    return [...this.world.people.values()].filter((reader) => named.has(reader.me.id)).map((reader) => userRef(reader.me));
  }

  private readers(person: Person, id: string): UserRef[] {
    if (!this.world.documents.readable(person).some((doc) => doc.id === id)) throw new Refusal(403, 'authz.denied', 'doc.view is required', { required: 'doc.view', missing: [] });
    return [...this.world.people.values()].filter((reader) => reader.me.kind === 'human' && !reader.suspended && this.world.documents.readable(reader).some((doc) => doc.id === id)).map((reader) => userRef(reader.me));
  }
  private noticeEnabled(person: Person): boolean {
    return !(this.world.notifications.prefs.get(person.me.username)?.disabled_kinds ?? []).some(isPersonNotice);
  }
  private chatActions(person: Person, target: Person, id: string, query: URLSearchParams): { canView: boolean; visible: boolean; gives: ('full_view' | 'full_interact')[] } {
    const chat = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {})?.body as Chat | undefined;
    if (chat === undefined) throw new Refusal(404, 'not_found', 'No current chat');
    const canView = chat.kind === 'node' ? this.world.places.access(target, chat.node?.id ?? '').permissions.includes('chat.view') : chat.members?.some((member) => member.id === target.me.id) === true;
    if (chat.kind === 'node' && chat.node !== null) {
      let page: PermissionPage;
      try { page = this.world.access.handle(person, 'GET', `/nodes/${chat.node.id}/permissions`, query, {})?.body as PermissionPage; }
      catch (error) { if (error instanceof Refusal) return { canView, visible: false, gives: [] }; throw error; }
      const local = this.world.places.isMember(target, chat.org_id);
      const can = (name: string) => page.rows.some((row) => row.permission === name && row.can_give);
      const gives: ('full_view' | 'full_interact')[] = [];
      if (!canView && (local || page.external) && can('chat.view')) {
        gives.push('full_view'); if (can('chat.interact')) gives.push('full_interact');
      }
      return { canView, visible: true, gives };
    }
    const canAdd = chat.kind === 'group' && chat.can.send && !chat.guests.includes(person.me.id)
      && (chat.members?.length ?? 0) < GROUP_MAX_MEMBERS && (this.world.places.isMember(target, chat.org_id) || this.world.access.ownersMayShareOutside && !this.world.access.noOutsiders.has(person.me.username));
    return { canView, visible: true, gives: !canView && canAdd ? ['full_interact'] : [] };
  }
  private choices(person: Person, chat: Chat, q: string): MentionChoice[] {
    const users = [...this.world.people.values()].filter((reader) => !reader.suspended && (chat.kind === 'node' ? this.world.places.access(reader, chat.node?.id ?? '').permissions.includes('chat.view') : chat.members?.some((member) => member.id === reader.me.id)));
    const base = { user:null, available: true, reason: null, thread: null, agent: null, state: null, started_at: null };
    const choices: MentionChoice[] = users.map((reader) => ({ ...base, target: { kind: 'user', id: reader.me.id }, name: reader.me.username, user: userRef(reader.me), count: reader.me.id === person.me.id ? 0 : 1 }));
    const broad = this.groupPolicy === 'writers' || chat.can.moderate || chat.created_by?.id === person.me.id;
    if (chat.kind !== 'direct' && users.length > 0) {
      const scope = chat.node?.id ?? chat.org_id;
      const count = users.filter((reader) => reader.me.id !== person.me.id && this.world.access.isHolder(reader, {kind:'role',role:'viewer',at:{id:scope,kind:'org',name:null,redacted:true},count:0},scope)).length;
      choices.push({ ...base, target: { kind: 'role', node_id: scope, role: 'viewer' }, name: 'Everyone here', count, available: broad, reason: broad ? null : 'Only managers may name roles and groups here. Acme decides.' });
      const developers = users.filter((reader) => reader.me.id !== person.me.id && this.world.access.isHolder(reader, {kind:'role',role:'developer',at:{id:scope,kind:'org',name:null,redacted:true},count:0},scope));
      if (developers.length > 0) choices.push({ ...base, target: { kind: 'role', node_id: chat.node?.id ?? chat.org_id, role: 'developer' }, name: 'Developers', count: developers.length, available: broad, reason: broad ? null : 'Only managers may name roles and groups here. Acme decides.' });
      const groups = this.world.access.handle(person, 'GET', `/orgs/${chat.org_id}/groups`, new URLSearchParams(), {})?.body as { id:string; name:string; description:string; org_id:string; member_count:number }[] | undefined;
      for (const group of groups ?? []) {
        const count = users.filter((reader) => reader.me.id !== person.me.id && this.world.access.isHolder(reader, {kind:'group',group},scope)).length;
        if (count > 0) choices.push({ ...base, target: { kind: 'group', id: group.id }, name: group.name, count, available: broad, reason: broad ? null : 'Only managers may name roles and groups here. Acme decides.' });
      }
    }
    const shared = new Set((this.agents.get(chat.id) ?? []).filter((agent) => agent.readable).map((agent) => agent.thread.id));
    const origins = [...this.world.trees.values()].flatMap((tree) => [...tree.nodes.values()]).filter((row) => row.kind === 'thread' && !row.deleted && (row.id === chat.node?.id || row.parent_id === chat.node?.id || shared.has(row.id)) && this.world.places.access(person,row.id).permissions.includes('thread.view'));
    for (const row of origins) {
      const thread = this.world.threads.thread(person,row);
      if (thread.binding === null) continue;
      choices.push({...base,target:{kind:'agent',thread_id:row.id},name:row.name,count:0,available:thread.input_access.allowed && !thread.archived,reason:thread.input_access.allowed ? null : 'thread.interact',thread:{id:row.id,kind:'thread',name:row.name,redacted:false},agent:thread.binding.agent.name,state:thread.state,started_at:thread.created_at});
    }
    return choices.filter((choice) => q === '' || choice.name.toLowerCase().includes(q.toLowerCase()) || choice.agent?.toLowerCase().includes(q.toLowerCase()));
  }

  private forwardParts(chat: Chat, messages: readonly ChatMessage[]): ContentPart[] {
    return copiesOfMessages(chat,messages);
  }

  /** Same thread preparation as ordinary input, with no acceptance, feed, notice or use written. */
  private askPlan(person: Person, chat: Chat, parts: ChatPart[], reply: string | null): {review:string;source:ChatMessage;asks:ChatAgentAskPreview[]} {
    const ids = [...new Set(parts.flatMap((part) => part.type === 'mention' && part.target.kind === 'agent' ? [part.target.thread_id] : []))].sort();
    if (ids.length > 10) throw new Refusal(400,'request.invalid','Choose at most ten agents');
    const original = reply === null ? undefined : this.world.chats.records.get(chat.id)?.messages.find((message) => message.id === reply && !message.deleted);
    if (reply !== null && original === undefined) throw new Refusal(400,'request.invalid','The original reply says nothing to forward');
    const source = messageRow(chat.seq+1,{id:randomUUID(),chat_id:chat.id,author:userRef(person.me),created_at:MADE_AT,parts:this.shown(parts,this.names(person,chat,parts))});
    const content = this.forwardParts(chat,[...(original === undefined ? [] : [original]),source]);
    const identities = ids.map((id) => {
      this.world.places.need(person,id,'thread.view');
      const row = this.world.places.rowOf(id);
      if (row?.kind !== 'thread') throw new Refusal(404,'not_found','No current thread');
      return this.world.threads.thread(person,row);
    });
    const review = createHash('sha256').update(JSON.stringify([person.me.id,parts,reply,original?.seq_updated,identities.map((thread) => [thread.id,thread.binding?.agent.id,thread.binding?.provider.id,thread.model,thread.effort])])).digest('hex');
    const asks = identities.map((thread): ChatAgentAskPreview => ({thread:{id:thread.id,kind:'thread',name:thread.name,redacted:false},agent:thread.binding?.agent.name ?? null,model:thread.model,effort:thread.effort,input:this.world.threads.prepareInput(person,thread.id,{content,delivery:'queue',client_id:`agent-preview:${source.id}`})}));
    return {review,source,asks};
  }
  projectChat(person: Person, chat: Chat): PeopleChat {
    let my: RevisedChatState | null = chat.my;
    const node = chat.node === null ? null : this.world.places.pathTo(chat.node.id).at(-1);
    const owner = node?.kind === 'thread' && this.world.places.metaOf(node.id).owner === person.me.username;
    const following = node?.kind === 'thread' && this.world.places.access(person, node.id).effective_subscription === 'all';
    if ((owner || following) && my === null) my = { chat_id: chat.id, level: null, engaged: false, effective_level: 'all', last_read_seq: 0, mentions_unread: 0, hidden: false, read_revision: 0, default_level: 'all', default_reason: owner ? 'owner' : 'following' };
    if (my !== null) {
      const default_level = owner || following || my.engaged || chat.kind !== 'node' ? 'all' : 'off';
      my = { ...my, default_level, default_reason: owner ? 'owner' : following ? 'following' : my.engaged ? 'engaged' : chat.kind !== 'node' ? 'member' : 'quiet', effective_level: my.level ?? default_level };
      this.world.chats.states.set(`${person.me.username}:${chat.id}`, my);
    }
    const agents = (this.agents.get(chat.id) ?? []).flatMap((agent) => {
      const origin = this.world.places.rowOf(agent.thread.id);
      const readable = origin !== undefined && this.world.places.access(person, origin.id).permissions.includes('thread.view');
      if (!readable) return [];
      const stopped = this.localStops.get(chat.id)?.has(agent.thread.id) === true || agent.stopped;
      return [{ ...agent, stopped, writable:agent.writable && !stopped, thread: { ...agent.thread, name: origin.name, redacted:false } }];
    });
    const inspection_holders = this.inspection ?? (chat.kind === 'node' ? [] : this.world.access.inspectionHolders(person, chat.org_id));
    return { ...chat, my, ...(chat.kind === 'node' ? {} : {inspection_holders}), agents };
  }
  private notice(person: Person, id: string, discussion: DocumentDiscussion, start: boolean): void {
    const doc = this.world.documents.docs.get(id);
    if (doc === undefined) return;
    const recipients = new Set<string>();
    if (start) {
      recipients.add(doc.owner);
      const by = discussion.block_id === null ? null : doc.blocks.get(discussion.block_id)?.updated_by;
      if (by !== null && by !== undefined) {
        if ('username' in by) recipients.add(by.username);
        else recipients.add(this.world.places.metaOf(by.thread.id).owner);
      }
    } else {
      recipients.add(discussion.created_by.username);
      for (const comment of discussion.comments) recipients.add(comment.author.username);
    }
    for (const [key, level] of this.levels) if (key.endsWith(`:${id}`) && level === 'all') recipients.add(key.slice(0, -id.length - 1));
    for (const username of recipients) {
      const reader = this.world.people.get(username);
      const inbox = this.world.notifications.control('/state', { user: username })?.body as { rows: { read_at: string | null; data: { discussion_id?: string } }[] };
      if (username === person.me.username || reader === undefined || inbox.rows.some((row) => row.read_at === null && row.data.discussion_id === discussion.id) || !this.world.documents.readable(reader).some((one) => one.id === id)) continue;
      this.world.notifications.add(username, { kind: 'doc.comment', title: `Discussion in ${doc.title}`, body: '', url: `/doc/${id}?discussion=${discussion.id}`, data: { document_id: id, discussion_id: discussion.id } });
    }
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>, key?: string): Answer | null {
    if (method === 'GET' && /^\/documents\/[^/]+\/notification-jobs$/.test(path)) {
      const id = path.split('/')[2] ?? '';
      this.world.documents.handle(person, 'GET', `/documents/${id}`, query, {});
      return { status: 200, body: { items: [], next_cursor: null } };
    }
    const imported = this.imports.handle(person, method, path, query, body);
    if (imported !== null) { this.requests.push({ method, path, body }); return imported; }
    let match: RegExpExecArray | null;
    if (method === 'POST' && /^\/orgs\/[^/]+\/chats$/.test(path)) {
      const answer = this.world.chats.handle(person, method, path, query, body);
      return answer === null ? null : { ...answer, body: this.projectChat(person, answer.body as Chat) };
    }
    if (method === 'GET' && path === '/me/state') {
      const original = this.world.settings.handle(person, method, path, query, body);
      return original === null ? null : { ...original, body: { ...original.body as object, reactions: this.reactions.get(person.me.username) ?? [] } };
    }
    if (method === 'PUT' && path === '/me/state/reactions') {
      const recent = Array.isArray(body) ? [...new Set(body.filter((value): value is string => typeof value === 'string'))].slice(0, RECENT_CHOICES) : [];
      this.reactions.set(person.me.username, recent); this.world.users.tell(person.me.username, { what: 'state', key: 'reactions', value: recent });
      return { status: 204 };
    }
    if (method === 'GET' && path === '/settings/registry') {
      const original = this.world.settings.handle(person, method, path, query, body);
      const defs: SettingDef[] = [
        { key: 'sharing.agent_documents', title: 'Documents for topic agents', text_key:null, keywords:[], description: 'What agents working here can do with public documents beside their topic and its main threads.', example:'read', group: 'agent', class: 'policy', scopes: ['system', 'org', 'project', 'topic'], merge: 'replace', default: 'write', schema: { type: 'string', enum: ['none', 'read', 'write'] } },
        { key: 'docs.templates', title: 'Start from these documents', text_key:null, keywords:[], description: 'Readable ordinary documents offered when creating a document here.', example:['00000000-0000-4000-8007-000000000001'], group: 'chat_documents', class: 'parameter', scopes: ['system', 'org', 'project', 'topic', 'thread'], merge: 'union', default: [], schema: { type: 'array', items: { type: 'string' } } },
      ];
      return original === null ? null : { ...original, body: [...(original.body as SettingDef[]).filter((def) => !defs.some((added) => added.key === def.key)), ...defs] };
    }
    if (method === 'PUT' && (match = /^\/nodes\/([^/]+)\/settings\/(sharing.agent_documents|docs.templates)$/.exec(path))) {
      const id = match[1] ?? ''; const key = match[2] ?? ''; this.world.places.need(person, id, 'settings.manage');
      if (key === 'sharing.agent_documents' && !['none', 'read', 'write'].includes(String(body.value))) throw new Refusal(400, 'request.invalid', 'Choose none, read or write');
      const previous = this.policyRows.get(`${id}:${key}`);
      this.policyRows.set(`${id}:${key}`, { value: body.value, revision: (previous?.revision ?? 0) + 1, locked: body.locked === true });
      this.requests.push({ method, path, body });
      return { status: 200, body: { revision: (previous?.revision ?? 0) + 1 } };
    }
    if (method === 'GET' && (match = /^\/nodes\/([^/]+)\/settings$/.exec(path))) {
      const id = match[1] ?? '';
      const original = this.world.settings.handle(person, method, path, query, body);
      if (original === null) return null;
      const extras: ResolvedSetting[] = ['sharing.agent_documents', 'docs.templates'].map((key) => {
        const scopes = [id, ...this.world.access.ancestorsOf(id)];
        const scope = scopes.find((scope) => this.policyRows.has(`${scope}:${key}`));
        const own = this.policyRows.get(`${id}:${key}`) ?? null;
        const stored = scope === undefined ? null : this.policyRows.get(`${scope}:${key}`) ?? null;
        const node = scope === undefined ? null : this.world.places.rowOf(scope);
        const ref = node === null || node === undefined ? null : { id: node.id, kind: node.kind, name: node.name, redacted: false };
        const lock = scopes.slice(1).find((scope) => this.policyRows.get(`${scope}:${key}`)?.locked === true);
        const lockNode = lock === undefined ? null : this.world.places.rowOf(lock);
        return { parts:[], key, value: stored?.value ?? (key === 'sharing.agent_documents' ? this.documentPolicy : this.templates), own, source: { scope: node?.kind ?? 'default', node: ref }, locked_by: lockNode === null || lockNode === undefined ? null : { scope: lockNode.kind, node: { id: lockNode.id, kind: lockNode.kind, name: lockNode.name, redacted: false } }, writable: lock === undefined && this.world.places.access(person, id).permissions.includes('settings.manage') };
      });
      return { ...original, body: [...(original.body as ResolvedSetting[]).filter((row) => !extras.some((extra) => extra.key === row.key)), ...extras] };
    }
    if (method === 'POST' && path === '/share/agents') {
      if (body.read_first === true && !(body.parts as { documents?: unknown } | undefined)?.documents) throw new Refusal(400, 'request.invalid', 'Read first needs document material');
      const original = this.world.access.shares.handle(person, method, path, query, body);
      if (original === null) return null;
      const shares = (original.body as { shares: AgentShare[] }).shares;
      for (const share of shares) this.priority.set(share.id, body.read_first === true);
      return { ...original, body: { shares: shares.map((share) => ({ ...share, read_first: this.priority.get(share.id) ?? false })) } };
    }
    if (method === 'GET' && /^\/(nodes|documents|chats)\/[^/]+\/agent-shares$/.test(path)) {
      const original = this.world.access.shares.handle(person, method, path, query, body);
      return original === null ? null : { ...original, body: (original.body as AgentShare[]).map((share) => ({ ...share, read_first: this.priority.get(share.id) ?? false })) };
    }
    if (method === 'GET' && /^\/threads\/([^/]+)\/shared$/.test(path)) {
      const original = this.world.access.shares.handle(person, method, path, query, body);
      if (original === null) return null;
      const given = original.body as { policy: string; shares: AgentShare[] };
      const shares = given.shares.map((share) => ({ ...share, read_first: this.priority.get(share.id) ?? false }));
      const readable = this.world.documents.readable(person);
      const first = shares.filter((share) => share.read_first).flatMap((share) => {
        const docs = readable.filter((doc) => doc.id === share.what.id || (doc.page !== 'restricted' && 'kind' in share.what && ['org', 'project', 'topic', 'thread'].includes(share.what.kind) && this.world.places.pathTo(doc.nodeId).some((node) => node.id === share.what.id)));
        return docs.map((doc) => ({ id: doc.id, path: `/${this.world.places.pathTo(doc.nodeId).map((node) => node.address ?? node.name).join('/')}/docs/${doc.title}.md` }));
      });
      return { ...original, body: { ...given, shares, documents_policy: this.documentPolicy, read_first_paths: first } };
    }
    if (method === 'GET' && (match = /^\/threads\/([^/]+)\/shared\/tree$/.exec(path))) {
      const threadId = match[1] ?? ''; this.world.places.need(person, threadId, 'thread.view');
      const thread = this.world.places.rowOf(threadId); const topicId = thread?.parent_id ?? '';
      const settings = this.handle(person, 'GET', `/nodes/${topicId}/settings`, query, {})?.body as ResolvedSetting[] | undefined;
      const policy = settings?.find((row) => row.key === 'sharing.agent_documents')?.value ?? this.documentPolicy;
      const explicit = this.world.access.shares.handle(person, 'GET', `/threads/${threadId}/shared`, query, {})?.body as { shares: AgentShare[] } | undefined;
      const entries = new Map<string, Record<string, unknown>>();
      for (const doc of this.world.documents.readable(person)) {
        const node = this.world.places.rowOf(doc.nodeId);
        const ownTopic = doc.nodeId === topicId || (node?.kind === 'thread' && node.parent_id === topicId);
        const share = explicit?.shares.find((share) => share.parts.documents !== undefined && (share.what.id === doc.id || this.world.places.pathTo(doc.nodeId).some((place) => place.id === share.what.id)));
        if (share === undefined && !(ownTopic && doc.page !== 'restricted' && policy !== 'none')) continue;
        const writable = share?.parts.documents === 'edit' || share?.parts.documents === 'manage' || (ownTopic && policy === 'write' && doc.page !== 'restricted');
        const base = { description: null, unit: null, parts: null, size: null, updated_at: doc.updatedAt, children: 1, readable: true, writable, manageable: share?.parts.documents === 'manage', deleted: false, via: share === undefined ? 'topic' : 'share', shared_by: share?.by ?? null, redacted: false };
        let path = '';
        for (const place of this.world.places.pathTo(doc.nodeId)) { path += `/${place.address ?? place.name}`; entries.set(`${path}/`, { ...base, path: `${path}/`, name: place.name, id: place.id, kind: place.kind }); }
        entries.set(`${path}/docs/`, { ...base, path: `${path}/docs/`, name: 'Documents', id: doc.nodeId, kind: 'folder' });
        entries.set(`${path}/docs/${doc.title}.md`, { ...base, path: `${path}/docs/${doc.title}.md`, name: doc.title, id: doc.id, kind: 'document', unit: 'block', parts: [...doc.blocks.values()].filter((block) => !block.removed).length, children: 0 });
      }
      const path = query.get('path') ?? '/';
      const prefix = path.endsWith('/') ? path : `${path}/`;
      const direct = [...entries.entries()].filter(([name]) => name.startsWith(prefix) && name !== prefix && !name.slice(prefix.length).replace(/\/$/, '').includes('/')).map(([, entry]) => entry);
      return { status: 200, body: { path, entries: direct, truncated: false } };
    }
    if ((match = /^\/chats\/([^/]+)\/mentions\/preview$/.exec(path)) && method === 'POST') {
      const answer = this.world.chats.handle(person, 'GET', `/chats/${match[1] ?? ''}`, query, {});
      if (answer === null) return null;
      const parts = Array.isArray(body.parts) ? body.parts as ChatPart[] : [];
      const chat = answer.body as Chat;
      if (!chat.can.send) throw new Refusal(403,'authz.denied','chat.interact is required');
      const count = this.audience(person, chat, parts.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n'), parts.flatMap((part) => part.type === 'mention' ? [part.target] : [])).length;
      if (!parts.some((part) => part.type === 'mention' && part.target.kind === 'agent')) return {status:200,body:{count}};
      const plan = this.askPlan(person,chat,parts,typeof body.reply_to === 'string' ? body.reply_to : null);
      return {status:200,body:{count,agent_asks:plan.asks,agent_review:plan.review}};
    }
    if ((match = /^\/chats\/([^/]+)\/read$/.exec(path)) && method === 'POST') {
      const id = match[1] ?? '';
      const answer = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {});
      if (answer === null) return null;
      const chat = this.projectChat(person, answer.body as Chat);
      if (chat.can.reason === 'inspecting') throw new Refusal(403, 'authz.denied', 'Inspection has no read state');
      const held = chat.my;
      const revision = held?.read_revision ?? 0;
      if (typeof body.read_revision !== 'number') throw new Refusal(400, 'request.invalid', 'The observed read revision is required');
      if (body.read_revision !== revision) throw new Refusal(409, 'conflict.stale_revision', 'The read position changed');
      const seq = Number(body.seq);
      const rec = this.world.chats.records.get(id);
      if (body.unread_from === true && !rec?.messages.some((message) => message.seq_created === seq && !message.deleted)) throw new Refusal(404, 'not_found', 'No live message there');
      const next: RevisedChatState = { chat_id: id, level: null, effective_level: 'off', engaged: false, hidden: false, mentions_unread: 0, ...held, read_revision: body.unread_from === true ? revision + 1 : revision, last_read_seq: body.unread_from === true ? Math.max(0, seq - 1) : Math.max(held?.last_read_seq ?? 0, seq) };
      next.mentions_unread = rec?.messages.filter((message) => message.seq_created > next.last_read_seq && message.mentions.some((user) => user.id === person.me.id)).length ?? 0;
      this.world.chats.states.set(`${person.me.username}:${id}`, next);
      this.world.users.tell(person.me.username, { what: 'chat_state', state: next });
      this.requests.push({ method, path, body });
      return { status: 204 };
    }
    if ((match = /^\/chats\/([^/]+)\/agents\/([^/]+)\/posting$/.exec(path)) && method === 'PUT') {
      const id = match[1] ?? ''; const threadId = match[2] ?? '';
      const answer = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {});
      if (answer === null) return null;
      const chat = answer.body as Chat;
      if (!chat.can.moderate && !(chat.kind === 'group' && chat.created_by?.id === person.me.id)) throw new Refusal(403, 'authz.denied', 'chat.manage is required', { required: 'chat.manage', missing: [] });
      const agents = this.agents.get(id) ?? [];
      const agent = agents.find((entry) => entry.thread.id === threadId);
      if (agent === undefined) throw new Refusal(404, 'not_found', 'No current origin');
      agent.stopped = body.stopped === true; this.requests.push({ method, path, body });
      const stops = this.localStops.get(id) ?? new Set<string>();
      if (agent.stopped) stops.add(threadId); else stops.delete(threadId);
      this.localStops.set(id, stops);
      return { status: 204 };
    }
    if ((match = /^\/chats\/([^/]+)\/messages$/.exec(path)) && method === 'POST') {
      const id = match[1] ?? '';
      const answer = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {});
      if (answer === null) return null;
      const chat = answer.body as Chat;
      if (!chat.can.send) throw new Refusal(403, 'authz.denied', 'chat.interact is required');
      const stable = `${person.me.id}:${id}:${String(body.client_id)}`;
      const previous = this.sent.get(stable); if (previous !== undefined) return {...previous,status:200};
      const parts = Array.isArray(body.parts) ? body.parts as ChatPart[] : [];
      const targets = parts.flatMap((part) => part.type === 'mention' ? [part.target] : []);
      const recipients = this.audience(person, chat, parts.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n'), targets);
      const names = this.names(person, chat, parts);
      const plan = targets.some((target) => target.kind === 'agent') ? this.askPlan(person,chat,parts,typeof body.reply_to === 'string' ? body.reply_to : null) : null;
      if (plan !== null && plan.review !== body.agent_review) throw new Refusal(409,'conflict.stale_revision','Review the current queued inputs before asking these agents');
      const result = this.world.chats.send(person,id,{...body,parts:this.shown(parts,names)},(message) => {
        message.mentions = recipients;
        message.mention_targets = targets; message.mention_names = names;
        if (plan !== null) message.asked_agents = plan.asks.map((ask) => {
          const content = ask.input.content.map((part) => part.type === 'forwarded' && part.source_id === plan.source.id ? {...part,source_id:message.id,at:message.created_at} : part);
          const accepted = this.world.threads.acceptInput(person,{...ask.input,content,client_id:`chat-mention:${message.id}`,created_at:message.created_at});
          return {thread:ask.thread,input_id:accepted.input.id};
        });
      });
      this.sent.set(stable, result);
      return result;
    }
    if ((match = /^\/chats\/([^/]+)\/messages\/([^/]+)$/.exec(path)) && method === 'PATCH') {
      const id = match[1] ?? '';
      const chat = this.world.chats.handle(person,'GET',`/chats/${id}`,query,{})?.body as Chat | undefined;
      if (chat === undefined) throw new Refusal(404,'not_found','No current chat');
      const parts = Array.isArray(body.parts) ? body.parts as ChatPart[] : [];
      const targets = parts.flatMap((part) => part.type === 'mention' ? [part.target] : []);
      const names = this.names(person,chat,parts);
      const recipients = this.audience(person,chat,parts.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n'),targets);
      return this.world.chats.edit(person,id,match[2] ?? '',{...body,parts:this.shown(parts,names)},(message) => { message.mentions=recipients; message.mention_targets=targets; message.mention_names=names; });
    }
    if ((match = /^\/chats\/([^/]+)\/draft$/.exec(path)) && (method === 'GET' || method === 'PUT')) {
      const id = match[1] ?? '';
      const chat = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {})?.body as Chat | undefined;
      if (chat === undefined) throw new Refusal(404, 'not_found', 'No current chat');
      const result = this.world.chats.handle(person, method, path, query, body);
      if (result === null || result.body === null) return result;
      const draft = result.body as {parts:ChatPart[]};
      return {...result,body:{...draft,mention_names:this.names(person,chat,draft.parts)}};
    }
    if ((match = /^\/([^/]+)\/([^/]+)\/notify\/people\/([^/]+)$/.exec(path)) && method === 'GET') {
      const object = match[1] ?? '', id = match[2] ?? '', user = match[3] ?? '';
      const target = [...this.world.people.values()].find((entry) => entry.me.id === user && !entry.suspended);
      if (target === undefined) throw new Refusal(404, 'not_found', 'No person');
      if (object === 'chats') {
        const actions = this.chatActions(person, target, id, query);
        return { status: 200, body: { can_view: actions.visible ? actions.canView : null, can_notify: actions.canView && this.noticeEnabled(target), gives: actions.gives, note_chars: 200 } };
      }
      const canView = object === 'documents' ? this.readers(person, id).some((entry) => entry.id === user)
        : object === 'nodes' ? this.world.places.access(target, id).permissions.some((permission) => permission.endsWith('.view'))
          : false;
      let page: PermissionPage | undefined;
      try { page = this.world.access.handle(person, 'GET', `/${object}/${id}/permissions`, query, {})?.body as PermissionPage | undefined; }
      catch (error) { if (!(error instanceof Refusal)) throw error; }
      return { status: 200, body: { can_view: page === undefined ? null : canView, can_notify: canView && this.noticeEnabled(target), gives: !canView ? page?.gives.choices.filter((choice) => choice !== 'full_manage') ?? [] : [], note_chars: 200 } };
    }
    if ((match = /^\/chats\/([^/]+)\/permissions\/people\/([^/]+)$/.exec(path)) && method === 'POST') {
      const id = match[1] ?? '', user = match[2] ?? '';
      const target = [...this.world.people.values()].find((entry) => entry.me.id === user && !entry.suspended);
      if (target === undefined) throw new Refusal(404, 'not_found', 'No person');
      const saved = key === undefined ? undefined : this.personActs.get(`${person.me.id}:${key}`);
      if (saved !== undefined) {
        if (saved.path !== path || saved.body !== JSON.stringify(body)) throw new Refusal(409, 'conflict.idempotency_mismatch', 'The key was used for another request');
        return { status: 204 };
      }
      const actions = this.chatActions(person, target, id, query);
      if (!actions.gives.some((choice) => choice === body.choice)) throw new Refusal(400, 'request.invalid', 'This choice is not available');
      if (body.notify === true && !this.noticeEnabled(target)) throw new Refusal(400, 'request.invalid', 'This person cannot be notified');
      const chat = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {})?.body as Chat;
      if (body.anchor !== undefined && body.anchor !== null) {
        const message = (body.anchor as { message?: unknown }).message;
        if (typeof message !== 'string' || !this.world.chats.records.get(id)?.messages.some((row) => row.id === message && !row.deleted)) throw new Refusal(400, 'request.invalid', 'The anchor is not a live message');
      }
      if (chat.kind === 'node' && chat.node !== null) {
        const permissions = body.choice === 'full_interact' ? ['chat.view', 'chat.interact'] : ['chat.view'];
        this.world.access.handle(person, 'PATCH', `/nodes/${chat.node.id}/permissions`, query, { rows: permissions.map((permission) => ({ permission, add: [{ holder: { kind: 'user', id: user } }] })) });
      } else this.world.chats.handle(person, 'PUT', `/chats/${id}/members/${user}`, query, {});
      if (body.notify === true) this.world.notifications.add(target.me.username, { kind: noticeKind('person.notify'), title: `${person.me.display_name} notified you`, url: `/chat/${id}`, data: { chat_id: id, by: person.me.id } });
      if (key !== undefined) this.personActs.set(`${person.me.id}:${key}`, { path, body: JSON.stringify(body) });
      this.requests.push({ method, path, body });
      return { status: 204 };
    }
    if ((match = /^\/documents\/([^/]+)\/permissions$/.exec(path)) && method === 'PATCH' && body.notify === true) {
      const id = match[1] ?? '';
      const saved = key === undefined ? undefined : this.personActs.get(`${person.me.id}:${key}`);
      if (saved !== undefined) {
        if (saved.path !== path || saved.body !== JSON.stringify(body)) throw new Refusal(409, 'conflict.idempotency_mismatch', 'The key was used for another request');
        return this.world.access.handle(person, 'GET', path, query, {});
      }
      const changes = Array.isArray(body.name) ? body.name : [];
      const change = changes.length === 1 ? changes[0] as { choice?: unknown; holders?: { kind?: unknown; id?: unknown }[] } : undefined;
      const holder = change?.holders?.length === 1 ? change.holders[0] : undefined;
      if (body.rows !== undefined || body.owners !== undefined || body.whole !== undefined || holder?.kind !== 'user' || typeof holder.id !== 'string' || (change?.choice !== 'full_view' && change?.choice !== 'full_interact')) throw new Refusal(400, 'request.invalid', 'Choose one person to grant and notify');
      const target = [...this.world.people.values()].find((entry) => entry.me.id === holder.id && entry.me.kind === 'human' && !entry.suspended);
      if (target === undefined || !this.noticeEnabled(target)) throw new Refusal(400, 'request.invalid', 'This person cannot be notified');
      const anchor = body.notify_anchor as { block?: unknown } | null | undefined;
      if (anchor !== undefined && anchor !== null && (typeof anchor.block !== 'string' || this.world.documents.docs.get(id)?.blocks.get(anchor.block)?.removed !== false)) throw new Refusal(400, 'request.invalid', 'The anchor is not a live block');
      const page = this.world.access.handle(person, 'PATCH', path, query, body);
      const url = `/doc/${id}${typeof anchor?.block === 'string' ? `?block=${anchor.block}` : ''}`;
      this.world.notifications.add(target.me.username, { kind: noticeKind('person.notify'), title: `${person.me.display_name} notified you`, url, data: { document_id: id, by: person.me.id, path: url } });
      if (key !== undefined) this.personActs.set(`${person.me.id}:${key}`, { path, body: JSON.stringify(body) });
      this.requests.push({ method, path, body });
      return page;
    }
    if ((match = /^\/([^/]+)\/([^/]+)\/notify$/.exec(path)) && method === 'POST') {
      const object = match[1] ?? '', id = match[2] ?? '';
      const saved = key === undefined ? undefined : this.personActs.get(`${person.me.id}:${key}`);
      if (saved !== undefined) {
        if (saved.path !== path || saved.body !== JSON.stringify(body)) throw new Refusal(409, 'conflict.idempotency_mismatch', 'The key was used for another request');
        return { status: 204 };
      }
      if (!Array.isArray(body.user_ids) || body.user_ids.length === 0 || !body.user_ids.every((value) => typeof value === 'string')) throw new Refusal(400, 'request.invalid', 'Choose people');
      if (body.note !== undefined && body.note !== null && (typeof body.note !== 'string' || Array.from(body.note).some((character) => LINE_BREAKS.has(character)) || Array.from(body.note).length > 200)) throw new Refusal(400, 'request.invalid', 'The note is too long or contains a newline');
      const readers = object === 'documents' ? this.readers(person, id) : [];
      const people = [...new Set(body.user_ids)].map((user) => [...this.world.people.values()].find((entry) => entry.me.id === user && !entry.suspended));
      if (people.some((target) => target === undefined || !this.noticeEnabled(target) || !(object === 'documents' ? readers.some((reader) => reader.id === target.me.id) : object === 'chats' ? this.chatActions(person, target, id, query).canView : object === 'nodes' && this.world.places.access(target, id).permissions.some((permission) => permission.endsWith('.view'))))) throw new Refusal(400, 'request.invalid', 'Each recipient must read the thing');
      const rawAnchor = body.anchor;
      if (rawAnchor !== undefined && rawAnchor !== null && typeof rawAnchor !== 'object') throw new Refusal(400, 'request.invalid', 'The anchor is not valid');
      const anchor = rawAnchor ?? undefined;
      const block = anchor !== undefined && 'block' in anchor ? anchor.block : undefined;
      const message = anchor !== undefined && 'message' in anchor ? anchor.message : undefined;
      if (anchor !== undefined && object === 'documents' && (typeof block !== 'string' || this.world.documents.docs.get(id)?.blocks.get(block)?.removed !== false)) throw new Refusal(400, 'request.invalid', 'The anchor is not a live block');
      if (anchor !== undefined && object === 'chats' && (typeof message !== 'string' || !this.world.chats.records.get(id)?.messages.some((row) => row.id === message && !row.deleted))) throw new Refusal(400, 'request.invalid', 'The anchor is not a live message');
      const url = object === 'documents' ? `/doc/${id}${typeof block === 'string' ? `?block=${block}` : ''}` : object === 'chats' ? `/chat/${id}${typeof message === 'string' ? `?message=${message}` : ''}` : `/topic/${id}`;
      for (const target of people) if (target !== undefined) this.world.notifications.add(target.me.username, { kind: noticeKind('person.notify'), title: `${person.me.display_name} notified you`, body: typeof body.note === 'string' ? body.note : '', url, data: { by: person.me.id, path: url, ...(object === 'documents' ? { document_id: id } : object === 'chats' ? { chat_id: id } : { target_node_id: id }) } });
      if (key !== undefined) this.personActs.set(`${person.me.id}:${key}`, { path, body: JSON.stringify(body) });
      this.requests.push({ method, path, body });
      return { status: 204 };
    }
    if ((match = /^\/documents\/([^/]+)\/mentions$/.exec(path)) && method === 'GET') {
      const q = (query.get('q') ?? '').toLowerCase();
      const id = match[1] ?? '';
      const readers = this.readers(person, id);
      const page = this.world.access.handle(person, 'GET', `/documents/${id}/permissions`, query, {})?.body as PermissionPage | undefined;
      const doc = this.world.documents.docs.get(id);
      const root = doc === undefined ? undefined : this.world.places.pathTo(doc.nodeId)[0]?.id;
      const sharedOrg = (other: Person) => [...this.world.places.roles.keys()].some((org) => this.world.places.isMember(person, org) && this.world.places.isMember(other, org));
      const people = page?.gives.choices.includes('full_view') === true ? [...this.world.people.values()].filter((other) => {
        const visible = other.me.username.toLowerCase() === q || sharedOrg(other) && (other.me.username.toLowerCase().startsWith(q) || other.me.display_name.toLowerCase().includes(q));
        return other.me.kind === 'human' && !other.suspended && visible && (page.external || root !== undefined && this.world.places.isMember(other, root));
      }).map((other) => userRef(other.me)).sort((a, b) => a.username.localeCompare(b.username) || a.id.localeCompare(b.id)) : readers;
      const seen = new Set(readers.map((user) => user.id));
      const items = [...readers.map((user) => ({ user, can_view: true })), ...people.filter((user) => !seen.has(user.id)).map((user) => ({ user, can_view: false }))]
        .filter(({ user }) => user.username.toLowerCase().includes(q) || user.display_name.toLowerCase().includes(q));
      const rawCursor = query.get('cursor');
      if (rawCursor !== null && !/^p:\d+$/.test(rawCursor)) throw new Refusal(400, 'request.invalid', 'The cursor is not a mention page cursor');
      const offset = rawCursor === null ? 0 : Number(rawCursor.slice(2));
      const rawLimit = query.get('limit');
      const requested = rawLimit === null ? 50 : Number(rawLimit);
      if (!Number.isSafeInteger(requested) || requested < 0 || !Number.isSafeInteger(offset)) throw new Refusal(400, 'request.invalid', 'The page size is not valid');
      const limit = Math.max(1, Math.min(200, requested));
      const end = Math.min(items.length, offset + limit);
      return { status: 200, body: { items: items.slice(offset, end), next_cursor: end < items.length ? `p:${end}` : null } };
    }
    if ((match = /^\/documents\/([^/]+)\/subscription$/.exec(path)) && method === 'PUT') {
      const id = match[1] ?? ''; this.readers(person, id);
      if (body.level !== 'all' && body.level !== 'involved') throw new Refusal(400, 'request.invalid', 'Choose a discussion level');
      this.levels.set(`${person.me.username}:${id}`, body.level); this.requests.push({ method, path, body });
      return { status: 204 };
    }
    if (method === 'GET' && /^\/documents\/[^/]+(?:\/sync)?$/.test(path)) {
      const answer = this.world.documents.handle(person, method, path, query, body);
      if (answer === null) return null;
      const id = path.split('/')[2] ?? '';
      const given = answer.body as Record<string, unknown>;
      const level = this.levels.get(`${person.me.username}:${id}`) ?? 'involved';
      return { ...answer, body: path.endsWith('/sync') ? { ...given, document: { ...given.document as object, discussion_level: level } } : { ...given, discussion_level: level } };
    }
    if (method === 'POST' && /^\/documents\/[^/]+\/discussions(?:\/[^/]+\/comments)?$/.test(path)) {
      const id = path.split('/')[2] ?? '';
      const readers = this.readers(person, id);
      const stable = typeof body.client_id === 'string' ? `${person.me.username}:${path}:${body.client_id}` : null;
      const previous = stable === null ? undefined : this.discussions.get(stable);
      if (previous !== undefined) return previous;
      const content = Array.isArray(body.content) ? body.content.map((run: { type?: string; user_id?: string }) => {
        if (run.type !== 'mention') return run;
        const user = readers.find((reader) => reader.id === run.user_id);
        if (user === undefined) throw new Refusal(403, 'authz.denied', 'Only current document readers can be mentioned');
        return { ...run, user };
      }) : [];
      const answer = this.world.documents.handle(person, method, path, query, { ...body, content });
      if (answer === null) return null;
      const start = path.endsWith('/discussions');
      const discussion = start ? answer.body as DocumentDiscussion : this.world.documents.docs.get(id)?.discussions.get(path.split('/')[4] ?? '');
      if (discussion !== undefined) {
        for (const comment of discussion.comments) comment.content = comment.content.map((run) => { if (run.type !== 'mention') return run; const user = readers.find((reader) => reader.id === run.user_id); return user === undefined ? run : { ...run, user }; });
        this.notice(person, id, discussion, start);
      }
      this.requests.push({ method, path, body });
      if (stable !== null) this.discussions.set(stable, answer);
      return answer;
    }
    if (method === 'POST' && /^\/documents\/[^/]+\/ops$/.test(path) && typeof body.expected_seq === 'number') {
      const doc = this.world.documents.docs.get(path.split('/')[2] ?? '');
      const repeated = doc?.done.has(`${person.me.username}:${String(body.client_id)}`) === true;
      if (!repeated && doc?.seq !== body.expected_seq) throw new Refusal(409, 'conflict.stale_revision', 'The document changed. Review it again.', { current: [...doc?.blocks.values() ?? []] });
      this.requests.push({ method, path, body });
      return this.world.documents.handle(person, method, path, query, body);
    }
    if ((match = /^\/chats\/([^/]+)\/mentions$/.exec(path)) && method === 'GET') {
      const id = match[1] ?? '';
      const answer = this.world.chats.handle(person, 'GET', `/chats/${id}`, query, {});
      if (answer === null) return null;
      const chat = this.projectChat(person, answer.body as Chat);
      const readers = new Set(this.choices(person,chat,'').flatMap((choice) => choice.target.kind === 'user' ? [choice.target.id] : []));
      const followers = this.followersOverride ?? [...this.world.people.values()].filter((reader) => readers.has(reader.me.id) && reader.me.id !== person.me.id && (this.world.chats.states.get(`${reader.me.username}:${id}`)?.effective_level ?? (chat.kind === 'node' ? 'off' : 'all')) !== 'off').length;
      return { status: 200, body: { items: this.choices(person, chat, query.get('q') ?? ''), next_cursor: null, followers } };
    }
    if (method === 'GET' && /^\/chats\/[^/]+(?:\/sync)?$/.test(path)) {
      const answer = this.world.chats.handle(person, method, path, query, body);
      if (answer === null) return null;
      const given = answer.body as { chat: Chat };
      return { ...answer, body: path.endsWith('/sync') ? { ...given, chat: this.projectChat(person, given.chat) } : this.projectChat(person, answer.body as Chat) };
    }
    return null;
  }
  control(path: string, body: Record<string, unknown>): Answer | null {
    if (path === '/agent-message') {
      const id = String(body.chat); const threadId = String(body.thread);
      const origin = this.agents.get(id)?.find((agent) => agent.thread.id === threadId);
      if (origin === undefined || !origin.writable || origin.stopped || this.localStops.get(id)?.has(threadId) === true) throw new Refusal(403, 'authz.denied', 'This agent cannot post here');
      const now = Date.now();
      const posts = (this.agentPosts.get(threadId) ?? []).filter((at) => at > now - HOUR_MS);
      if (posts.length >= AGENT_POSTS_PER_HOUR) throw new Refusal(429, 'rate_limited', 'The agent can post again later', { resets_at: new Date((posts[0] ?? now) + HOUR_MS).toISOString() });
      const message = this.world.chats.postAgent(id, origin.agent, origin.thread, String(body.text));
      this.agentPosts.set(threadId, [...posts, now]);
      if (posts.length + 1 >= AGENT_POSTS_PER_HOUR) origin.next_allowed_at = new Date((posts[0] ?? now) + HOUR_MS).toISOString();
      return { status: 200, body: message };
    }
    if (path === '/policy') { this.documentPolicy = typeof body.documents === 'string' ? body.documents : 'write'; if (Array.isArray(body.templates)) this.templates = body.templates as string[]; return { status: 204 }; }
    if (path === '/agents') { this.agents.set(String(body.chat), (body.agents as NonNullable<PeopleChat['agents']>).map((agent) => ({...agent,next_allowed_at:agent.next_allowed_at ?? null}))); return { status: 204 }; }
    if (path === '/configure') { this.groupPolicy = body.group_policy === 'managers' ? 'managers' : 'writers'; this.followersOverride = typeof body.followers === 'number' ? body.followers : null; if (Array.isArray(body.inspection)) this.inspection = body.inspection as typeof this.inspection; return { status: 204 }; }
    if (path === '/requests') return { status: 200, body: this.requests };
    if (path === '/levels') return { status: 200, body: [...this.levels] };
    return null;
  }
}
