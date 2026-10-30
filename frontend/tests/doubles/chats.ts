/** The fake API of chats: a node's chat and the direct and group chats of a root org, their messages, reactions and pins, read positions, drafts, forwarding, and the chat feed with its typing and presence.
 *
 * It keeps to what the real API promises of these requests as far as a screen can tell: the same
 * members, the same codes, the same rules of who may read, write and moderate. A node's chat exists
 * for every node of a tree as soon as the tree does. A person may be a guest of a group: they are in
 * that chat and nowhere else of the tree. Everything is in memory and is forgotten when the world
 * is reset. The requests only the double has, which steer it, are under `/__double/chats`.
 */
import { randomUUID } from 'node:crypto';

import type {
  Chat,
  ChatDraft,
  ChatMessage,
  ChatPart,
  ChatRef,
  ChatState,
  ContentPart,
  Item,
  NodeRef,
  TreeNode,
  UserRef,
} from '../../src/generated/api-types.ts';
import { me as makeMe, userRef } from './fixtures.ts';
import { agentMessage, userMessage } from './thread-rows.ts';
import { type Answer, type Part, type Person, Refusal, type Subscriber, type World } from './world.ts';

/** The numbers the real contract states. */
const MESSAGES_DEFAULT = 100;
const MESSAGES_MAX = 500;
const SYNC_MAX_CHANGES = 2_000;
const FORWARD_MAX_MESSAGES = 20;
const FORWARD_MAX_ITEMS = 200;
const FORWARD_MAX_DESTINATIONS = 10;
const GROUP_MAX_MEMBERS = 12;
const REPLY_EXCERPT_CHARS = 120;
const REACTION_KINDS_MAX = 20;
const PAGE = 50;
const MENTION = /(^|[^\w@])@([a-z0-9][a-z0-9_-]{2,31}|all)\b/gi;

type Kind = 'node' | 'direct' | 'group';

interface Rec {
  id: string;
  kind: Kind;
  /** The root org it belongs to. */
  orgId: string;
  nodeId: string | null;
  name: string | null;
  /** The people in a direct or group chat, by username, including guests. */
  members: string[];
  guests: Set<string>;
  createdBy: string | null;
  createdAt: string;
  seq: number;
  lastMessageSeq: number;
  lastMessageAt: string | null;
  messages: ChatMessage[];
  /** Messages by the person and the key of the act, for the repeat of a send. */
  sent: Map<string, string>;
}

/** Somebody listening to a chat's feed. */
interface Follower {
  subscriber: Subscriber;
  id: number;
  username: string;
  inspecting: boolean;
  visible: boolean;
  since: string;
  lastSent: number;
}

interface Standing {
  reads: boolean;
  sends: boolean;
  moderates: boolean;
  inspects: boolean;
  reason: 'archived' | 'disabled' | 'closed' | 'no_permission' | 'inspecting' | null;
}

const now = (): string => new Date().toISOString();
const asText = (value: unknown): string => (typeof value === 'string' ? value : '');

/** A copy made by forwarding: on the wire it says `type: 'forwarded'`, which the generated types leave out of it. */
export function isForwarded(part: ChatPart): part is Extract<ChatPart, { type: 'forwarded' }> {
  return 'type' in part && part.type === 'forwarded';
}

/** Ordinary chat forwarding and the deliberate ask preview carry the same source snapshots. */
export function copiesOfMessages(chat: ChatRef, messages: readonly ChatMessage[]): Extract<ChatPart, {type:'forwarded'}>[] {
  return messages.flatMap((message) => {
    const words = message.parts.flatMap((part) => part.type === 'text' ? [part.text] : []).join('\n');
    const images = message.parts.flatMap((part) => part.type === 'image' ? [{blob:part.blob,media_type:part.media_type,name:part.name}] : []);
    const attachments = message.parts.flatMap((part) => part.type === 'attachment' ? [{blob:part.blob,media_type:part.media_type,name:part.name,size:part.size}] : []);
    const own: Extract<ChatPart,{type:'forwarded'}>[] = words.trim() !== '' || images.length > 0 || attachments.length > 0 ? [{type:'forwarded',source_id:message.id,from:{kind:'chat',chat},author:message.author !== null && 'agent' in message.author ? {agent:message.author.agent} : message.author,at:message.created_at,text:words,images,attachments}] : [];
    return [...own,...message.parts.filter(isForwarded)];
  });
}

/** The text of a message as a copy, a reply excerpt or a search holds it. */
export function textOf(parts: readonly ChatPart[]): string {
  return parts
    .map((part) => {
      if (isForwarded(part)) return part.text;
      if (part.type === 'text') return part.text;
      if (part.type === 'attachment' || part.type === 'image') return part.name;
      return '';
    })
    .filter((text) => text !== '')
    .join('\n');
}

/** What an item of a thread says, as the Markdown a copy of it holds: what people and the agent said; any other kind says nothing here. */
function wordsOfItem(item: Item): string {
  if (item.kind === 'agent_message') return item.payload.text;
  if (item.kind === 'user_message') return item.payload.parts.map((part) => ('source_id' in part ? part.text : part.type === 'text' ? part.text : part.name)).join('\n');
  return '';
}

function invalid(path: string, message: string): never {
  throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
}

function notFound(): never {
  throw new Refusal(404, 'not_found', 'no such chat');
}

export class Chats implements Part {
  readonly world: World;
  records = new Map<string, Rec>();
  states = new Map<string, ChatState>();
  drafts = new Map<string, ChatDraft>();
  private forwards = new Map<string, Answer>();
  /** Node chats switched off (`chat.enabled`), by chat. */
  switchedOff = new Set<string>();
  /** Chats whose next frame to followers is "lost", to see a gap repaired. */
  lose = new Set<string>();
  followers = new Map<string, Follower[]>();
  /** The newest typing frame sent for a chat and a person, to let one in three seconds through as the real one does. */
  typingAt = new Map<string, number>();
  constructor(world: World) {
    this.world = world;
  }

  // ---------------------------------------------------------------- who is who

  private person(username: string): Person | undefined {
    return this.world.people.get(username);
  }

  private ref(username: string): UserRef {
    const person = this.person(username);
    if (person === undefined) {
      return { id: randomUUID(), kind: 'human', username, display_name: username, avatar: { text: username.slice(0, 1).toUpperCase(), color: 0 } };
    }
    return userRef(person.me);
  }

  private nodeRows(): { root: string; row: TreeNode }[] {
    const rows: { root: string; row: TreeNode }[] = [];
    for (const [root, tree] of this.world.trees) for (const row of tree.nodes.values()) rows.push({ root, row });
    return rows;
  }

  private isMember(person: Person, rootId: string): boolean {
    return person.me.is_system_admin || this.world.places.isMember(person, rootId);
  }

  private isAdmin(person: Person, rootId: string): boolean {
    return person.me.is_system_admin || this.world.places.roleAt(person, rootId) === 'admin';
  }

  // ---------------------------------------------------------------- the chats there are

  /** The chat with this id, made from the node that owns it when it is the chat of a node. */
  private rec(chatId: string): Rec {
    const held = this.records.get(chatId);
    if (held !== undefined) return held;
    for (const { root, row } of this.nodeRows()) {
      if (row.chat.id !== chatId) continue;
      const made: Rec = {
        id: chatId,
        kind: 'node',
        orgId: root,
        nodeId: row.id,
        name: null,
        members: [],
        guests: new Set(),
        createdBy: this.world.places.metaOf(row.id).owner,
        createdAt: this.world.places.metaOf(row.id).createdAt,
        seq: 0,
        lastMessageSeq: row.chat.seq,
        lastMessageAt: null,
        messages: [],
        sent: new Map(),
      };
      this.records.set(chatId, made);
      return made;
    }
    return notFound();
  }

  private standing(person: Person, rec: Rec): Standing {
    const username = person.me.username;
    if (rec.kind === 'node') {
      const row = this.world.trees.get(rec.orgId)?.nodes.get(rec.nodeId ?? '');
      const permissions = this.world.places.access(person, rec.nodeId ?? '').permissions;
      const reads = row !== undefined && permissions.includes('chat.view');
      const archived = row?.archived === true;
      const off = this.switchedOff.has(rec.id);
      const sends = reads && permissions.includes('chat.interact') && !archived && !off;
      return {
        reads,
        sends,
        moderates: sends && permissions.includes('chat.manage'),
        inspects: false,
        reason: !reads ? 'no_permission' : archived ? 'archived' : off ? 'disabled' : sends ? null : 'no_permission',
      };
    }
    const inside = rec.members.includes(username);
    if (inside) {
      const closed = rec.kind === 'direct' && rec.members.some((other) => other !== username && this.person(other)?.suspended === true);
      return { reads: true, sends: !closed, moderates: false, inspects: false, reason: closed ? 'closed' : null };
    }
    const inspects = this.isAdmin(person, rec.orgId);
    return { reads: inspects, sends: false, moderates: false, inspects, reason: inspects ? 'inspecting' : 'no_permission' };
  }

  /** The chat as the caller may read it; a direct or group chat they may not read does not exist for them. */
  private readable(person: Person, chatId: string): { rec: Rec; standing: Standing } {
    const rec = this.rec(chatId);
    const standing = this.standing(person, rec);
    if (!standing.reads) {
      if (rec.kind !== 'node') notFound();
      throw new Refusal(403, 'authz.denied', 'chat.view is required here', { required: 'chat.view', requestable: true, override: false, missing: [] });
    }
    return { rec, standing };
  }

  private writable(person: Person, chatId: string): { rec: Rec; standing: Standing } {
    const found = this.readable(person, chatId);
    if (found.standing.inspects) {
      throw new Refusal(403, 'authz.denied', 'an admin who reads a chat they are not in only reads it', { required: 'chat.interact', requestable: false, override: false, missing: [] });
    }
    if (!found.standing.sends) {
      const reason = found.standing.reason ?? 'no_permission';
      throw new Refusal(403, 'chat.not_allowed', `this chat does not allow it (${reason})`, { reason });
    }
    return found;
  }

  private nodeRef(rec: Rec): NodeRef | null {
    if (rec.nodeId === null) return null;
    const row = this.world.trees.get(rec.orgId)?.nodes.get(rec.nodeId);
    return row === undefined ? null : { id: row.id, kind: row.kind, name: row.name, redacted: false };
  }

  private stateOf(username: string, chatId: string): ChatState | null {
    return this.states.get(`${username}:${chatId}`) ?? null;
  }

  private ensureState(username: string, rec: Rec, engaged = false): ChatState {
    const key = `${username}:${rec.id}`;
    const held = this.states.get(key);
    const base: ChatState = held ?? {
      chat_id: rec.id,
      effective_level: 'off',
      engaged: false,
      hidden: false,
      last_read_seq: 0,
      read_revision: 0,
      level: null,
      mentions_unread: 0,
    };
    const next: ChatState = { ...base, engaged: base.engaged || engaged };
    next.effective_level = next.level ?? (next.engaged || (rec.kind !== 'node' && rec.members.includes(username)) ? 'all' : 'off');
    this.states.set(key, next);
    return next;
  }

  /** The chat as the caller reads it. */
  view(person: Person, rec: Rec): Chat {
    const standing = this.standing(person, rec);
    const own = rec.kind === 'node' ? this.stateOf(person.me.username, rec.id) : this.ensureState(person.me.username, rec);
    const last = rec.messages.findLast((message) => message.kind === 'message');
    const words = last?.parts.flatMap((part) => part.type === 'text' || part.type === 'forwarded' ? [part.text.trim()] : []).filter(Boolean).join(' ');
    const carried = last?.parts.flatMap((part) => part.type === 'image' ? [`[image: ${part.name}]`] : part.type === 'attachment' ? [`[file: ${part.name}]`] : []).join(' ');
    return {
      id: rec.id,
      kind: rec.kind,
      org_id: rec.orgId,
      node: this.nodeRef(rec),
      name: rec.name,
      members: rec.kind === 'node' ? null : rec.members.map((username) => this.ref(username)),
      guests: [...rec.guests].map((username) => this.ref(username).id),
      created_by: rec.createdBy === null ? null : this.ref(rec.createdBy),
      created_at: rec.createdAt,
      seq: rec.seq,
      last_message_seq: rec.lastMessageSeq,
      last_message_at: rec.lastMessageAt,
      preview: last === undefined ? null : {
        id: last.id, author: last.author, deleted: last.deleted,
        text: last.deleted ? '' : Array.from(words === undefined || words === '' ? carried ?? '' : words).slice(0, 120).join(''),
      },
      // A chat of a node is taken away by the right its node's page gives; a direct or a group chat by being in it, and not by an admin who only reads it.
      can: { admin_only: [], participating: standing.reads && !standing.inspects && ((own?.effective_level ?? 'off') !== 'off' || rec.nodeId !== null && this.world.places.participates(person, rec.nodeId)), send: standing.sends, moderate: standing.moderates, export: rec.kind === 'node' ? rec.nodeId !== null && this.world.places.access(person, rec.nodeId).permissions.includes('chat.export') : !standing.inspects, reason: standing.sends ? null : standing.reason },
      my: standing.inspects ? null : own,
    };
  }

  // ---------------------------------------------------------------- requests

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    if (route === 'GET /me/chat-states') {
      const list = [...this.states.entries()].filter(([key]) => key.startsWith(`${person.me.username}:`)).map(([, state]) => state);
      return { status: 200, body: list };
    }
    if (route === 'POST /forward') return this.forward(person, body);

    if ((found = /^GET \/orgs\/([^/]+)\/chats$/.exec(route))) return { status: 200, body: this.list(person, found[1] ?? '', query) };
    if ((found = /^POST \/orgs\/([^/]+)\/chats$/.exec(route))) return this.make(person, found[1] ?? '', body);
    if ((found = /^POST \/orgs\/([^/]+)\/chats\/read$/.exec(route))) return this.readAll(person, found[1] ?? '');

    if ((found = /^GET \/chats\/([^/]+)$/.exec(route))) {
      const { rec } = this.readable(person, found[1] ?? '');
      return { status: 200, body: this.view(person, rec) };
    }
    if ((found = /^PATCH \/chats\/([^/]+)$/.exec(route))) return this.rename(person, found[1] ?? '', body);
    if ((found = /^(PUT|DELETE) \/chats\/([^/]+)\/members\/([^/]+)$/.exec(route))) {
      return found[1] === 'PUT' ? this.addMember(person, found[2] ?? '', found[3] ?? '') : this.removeMember(person, found[2] ?? '', found[3] ?? '');
    }
    if ((found = /^GET \/chats\/([^/]+)\/sync$/.exec(route))) return { status: 200, body: this.sync(person, found[1] ?? '', query) };
    if ((found = /^GET \/chats\/([^/]+)\/messages$/.exec(route))) return { status: 200, body: this.messages(person, found[1] ?? '', query) };
    if ((found = /^POST \/chats\/([^/]+)\/messages$/.exec(route))) return this.send(person, found[1] ?? '', body);
    if ((found = /^PATCH \/chats\/([^/]+)\/messages\/([^/]+)$/.exec(route))) return this.edit(person, found[1] ?? '', found[2] ?? '', body);
    if ((found = /^DELETE \/chats\/([^/]+)\/messages\/([^/]+)$/.exec(route))) return this.remove(person, found[1] ?? '', found[2] ?? '');
    if ((found = /^(PUT|DELETE) \/chats\/([^/]+)\/messages\/([^/]+)\/reactions\/(.+)$/.exec(route))) {
      return this.react(person, found[2] ?? '', found[3] ?? '', decodeURIComponent(found[4] ?? ''), found[1] === 'PUT');
    }
    if ((found = /^(PUT|DELETE) \/chats\/([^/]+)\/messages\/([^/]+)\/pin$/.exec(route))) {
      return this.pin(person, found[2] ?? '', found[3] ?? '', found[1] === 'PUT');
    }
    if ((found = /^POST \/chats\/([^/]+)\/read$/.exec(route))) return this.read(person, found[1] ?? '', body);
    if ((found = /^PUT \/chats\/([^/]+)\/state$/.exec(route))) return this.setState(person, found[1] ?? '', body);
    if ((found = /^GET \/chats\/([^/]+)\/draft$/.exec(route))) {
      const { standing } = this.readable(person, found[1] ?? '');
      if (standing.inspects) throw new Refusal(403, 'authz.denied', 'an inspecting reader has no private draft here');
      return { status: 200, body: this.drafts.get(`${person.me.username}:${found[1] ?? ''}`) ?? null };
    }
    if ((found = /^PUT \/chats\/([^/]+)\/draft$/.exec(route))) return this.saveDraft(person, found[1] ?? '', body);
    if ((found = /^DELETE \/chats\/([^/]+)\/draft$/.exec(route))) {
      this.writable(person, found[1] ?? '');
      this.drafts.delete(`${person.me.username}:${found[1] ?? ''}`);
      this.tellUser(person.me.username, { what: 'chat_draft', chat_id: found[1] ?? '', draft: null });
      return { status: 204 };
    }
    if (/^POST \/chats\/[^/]+\/attachments$/.test(route) || /^GET \/chats\/[^/]+\/blobs\/[^/]+$/.test(route)) {
      throw new Refusal(404, 'not_found', 'the double does not store files');
    }
    return null;
  }

  // ---------------------------------------------------------------- what finds things reads

  /**
   * The chats a person may read, each as the API refers to it, with the places above it and what was said in it, and whether the person is in it
   * (a group or a direct chat they belong to: a place's chat is the place's, and is found by the place): for the finder's names and for the search.
   */
  forSearch(person: Person): { chat: ChatRef; label: string; path: NodeRef[]; messages: ChatMessage[]; in: boolean }[] {
    const found: { chat: ChatRef; label: string; path: NodeRef[]; messages: ChatMessage[]; in: boolean }[] = [];
    const said = (rec: Rec) => rec.messages.filter((message) => message.kind === 'message' && !message.deleted);
    for (const { row } of this.nodeRows()) {
      if (row.deleted) continue;
      const rec = this.rec(row.chat.id);
      if (!this.standing(person, rec).reads) continue;
      const path = this.world.places.pathTo(row.id).map((above): NodeRef => ({ id: above.id, kind: above.kind, name: above.name, redacted: false }));
      found.push({ chat: { id: rec.id, kind: 'node', name: null, node: this.nodeRef(rec), org_id: rec.orgId }, label: row.name, path, messages: said(rec), in: false });
    }
    for (const rec of this.records.values()) {
      if (rec.kind === 'node' || !rec.members.includes(person.me.username)) continue;
      found.push({ chat: { id: rec.id, kind: rec.kind, name: rec.name, node: null, org_id: rec.orgId }, label: rec.name ?? [...rec.members].sort().map((username) => this.ref(username).display_name).join(', '), path: [], messages: said(rec), in: true });
    }
    return found;
  }

  // ---------------------------------------------------------------- lists

  private page<T>(items: T[], query: URLSearchParams): { items: T[]; next_cursor: string | null } {
    const limit = Math.min(Number(query.get('limit') ?? PAGE) || PAGE, 200);
    const from = Number(query.get('cursor') ?? 0) || 0;
    const slice = items.slice(from, from + limit);
    return { items: slice, next_cursor: from + limit < items.length ? String(from + limit) : null };
  }

  private recent(a: Rec, b: Rec): number {
    return (b.lastMessageAt ?? b.createdAt).localeCompare(a.lastMessageAt ?? a.createdAt);
  }

  private list(person: Person, orgId: string, query: URLSearchParams) {
    const root = this.world.trees.get(orgId);
    if (root === undefined) return notFound();
    const username = person.me.username;
    const scope = query.get('scope') ?? 'mine';
    const guestOf = [...this.records.values()].filter((rec) => rec.orgId === orgId && rec.kind === 'group' && rec.guests.has(username));
    const member = this.isMember(person, orgId);
    if (!member && guestOf.length === 0) return notFound();
    const people = [...this.records.values()].filter((rec) => rec.orgId === orgId && rec.kind !== 'node');
    if (scope === 'mine') {
      const mine = people.filter((rec) => rec.members.includes(username) && this.stateOf(username, rec.id)?.hidden !== true);
      return this.page(mine.sort((a, b) => this.recent(a, b)).map((rec) => this.view(person, rec)), query);
    }
    if (!member) throw new Refusal(403, 'authz.denied', 'directs.view is required', { required: 'directs.view', requestable: false, override: false, missing: [] });
    if (scope === 'others') {
      const of = query.get('of') ?? orgId;
      if (!this.isAdmin(person, orgId) && !this.isAdmin(person, of)) {
        throw new Refusal(403, 'authz.denied', 'directs.view is required', { required: 'directs.view', requestable: false, override: false, missing: [] });
      }
      const others = people.filter((rec) => !rec.members.includes(username) && this.inspectableOf(rec, of, orgId));
      return this.page(others.sort((a, b) => this.recent(a, b)).map((rec) => this.view(person, rec)), query);
    }
    // The chats of nodes that have messages and that the caller reads.
    const nodes = [...this.world.trees.get(orgId)?.nodes.values() ?? []]
      .map((row) => this.rec(row.chat.id))
      .filter((rec) => rec.messages.length > 0 && this.standing(person, rec).reads);
    return this.page(nodes.sort((a, b) => this.recent(a, b)).map((rec) => this.view(person, rec)), query);
  }

  /** Whether the people of a direct or group chat are all members of the org `of` (the root org: all of them, guests too). */
  private inspectableOf(rec: Rec, of: string, root: string): boolean {
    if (of === root) return true;
    return rec.members.every((username) => !rec.guests.has(username) && this.world.places.roles.get(root)?.get(username)?.domain === of);
  }

  // ---------------------------------------------------------------- making and changing chats

  private make(person: Person, orgId: string, body: Record<string, unknown>): Answer {
    if (!this.world.trees.has(orgId) || !this.isMember(person, orgId)) return notFound();
    const username = person.me.username;
    const ids = Array.isArray(body.user_ids) ? body.user_ids.filter((one): one is string => typeof one === 'string') : [];
    const name = typeof body.name === 'string' && body.name.trim() !== '' ? body.name.trim() : null;
    if (name !== null && Array.from(name).length > 80) invalid('name', 'must be at most 80 characters');
    if (ids.length === 0) invalid('user_ids', 'must name at least one other person');
    const others = ids.map((id) => [...this.world.people.values()].find((one) => one.me.id === id)).filter((one): one is Person => one !== undefined);
    if (others.length !== ids.length) invalid('user_ids', 'names somebody who does not exist');
    if (others.some((one) => one.me.username === username)) invalid('user_ids', 'must not name the person who makes the chat');
    if (ids.length === 1 && name === null) {
      const other = others[0];
      if (other === undefined || !this.isMember(other, orgId)) return invalid('user_ids', 'a direct chat is between two members of the org');
      const made = [...this.records.values()].find(
        (rec) => rec.kind === 'direct' && rec.orgId === orgId && rec.members.includes(username) && rec.members.includes(other.me.username),
      );
      if (made !== undefined) {
        const state = this.ensureState(username, made);
        this.states.set(`${username}:${made.id}`, { ...state, hidden: false });
        return { status: 200, body: this.view(person, made) };
      }
      const direct = this.newRec('direct', orgId, [username, other.me.username], null, username);
      this.ensureState(username, direct);
      this.ensureState(other.me.username, direct);
      return { status: 201, body: this.view(person, direct) };
    }
    if (ids.length + 1 > GROUP_MAX_MEMBERS) throw new Refusal(403, 'chat.not_allowed', 'this chat does not allow it (full)', { reason: 'full' });
    const outsiders = others.filter((one) => !this.isMember(one, orgId));
    if (outsiders.length > 0 && !this.isAdmin(person, orgId) && this.world.places.roleAt(person, orgId) !== 'developer') {
      throw new Refusal(403, 'sharing.external_forbidden', 'only a Developer may bring in somebody from outside', { ask: [], ask_more: 0 });
    }
    const group = this.newRec('group', orgId, [username, ...others.map((one) => one.me.username)], name, username);
    for (const outsider of outsiders) group.guests.add(outsider.me.username);
    for (const member of group.members) this.ensureState(member, group);
    this.system(group, 'created', username, null, name);
    this.afterChange(group);
    return { status: 201, body: this.view(person, group) };
  }

  private newRec(kind: Kind, orgId: string, members: string[], name: string | null, by: string): Rec {
    const rec: Rec = {
      id: randomUUID(),
      kind,
      orgId,
      nodeId: null,
      name,
      members,
      guests: new Set(),
      createdBy: by,
      createdAt: now(),
      seq: 0,
      lastMessageSeq: 0,
      lastMessageAt: null,
      messages: [],
      sent: new Map(),
    };
    this.records.set(rec.id, rec);
    for (const member of members) {
      this.tellUser(member, { what: 'chat', chat_id: rec.id, seq: 0, at: rec.createdAt });
    }
    return rec;
  }

  private rename(person: Person, chatId: string, body: Record<string, unknown>): Answer {
    const { rec } = this.writable(person, chatId);
    if (rec.kind !== 'group') throw new Refusal(403, 'chat.not_allowed', 'only a group has a name', { reason: 'not_a_group' });
    const name = body.name === null ? null : asText(body.name).trim();
    if (name !== null && (name === '' || Array.from(name).length > 80)) invalid('name', 'must be 1 to 80 characters');
    rec.name = name;
    this.system(rec, 'renamed', person.me.username, null, name);
    this.afterChange(rec);
    return { status: 200, body: this.view(person, rec) };
  }

  private addMember(person: Person, chatId: string, userId: string): Answer {
    const { rec } = this.writable(person, chatId);
    if (rec.kind !== 'group') throw new Refusal(403, 'chat.not_allowed', 'people are added to a group', { reason: 'not_a_group' });
    const other = [...this.world.people.values()].find((one) => one.me.id === userId);
    if (other === undefined) return notFound();
    if (rec.members.length >= GROUP_MAX_MEMBERS) throw new Refusal(403, 'chat.not_allowed', 'this chat does not allow it (full)', { reason: 'full' });
    if (!this.isMember(other, rec.orgId)) rec.guests.add(other.me.username);
    if (!rec.members.includes(other.me.username)) rec.members.push(other.me.username);
    this.ensureState(other.me.username, rec);
    this.system(rec, 'member_added', person.me.username, other.me.username, null);
    this.tellUser(other.me.username, { what: 'chat', chat_id: rec.id, seq: rec.lastMessageSeq, at: rec.lastMessageAt ?? rec.createdAt });
    this.afterChange(rec);
    return { status: 200, body: this.view(person, rec) };
  }

  private removeMember(person: Person, chatId: string, userId: string): Answer {
    const { rec } = this.readable(person, chatId);
    const other = [...this.world.people.values()].find((one) => one.me.id === userId);
    if (rec.kind !== 'group' || other === undefined || !rec.members.includes(other.me.username)) return notFound();
    const self = other.me.username === person.me.username;
    if (!self && rec.createdBy !== person.me.username) {
      throw new Refusal(403, 'authz.denied', 'only the person who made the group removes others', { required: 'chat.interact', requestable: false, override: false, missing: [] });
    }
    rec.members = rec.members.filter((username) => username !== other.me.username);
    rec.guests.delete(other.me.username);
    this.system(rec, self ? 'member_left' : 'member_removed', person.me.username, other.me.username, null);
    this.afterChange(rec);
    this.closeFor(rec, other.me.username);
    return { status: 204 };
  }

  // ---------------------------------------------------------------- messages

  private sync(person: Person, chatId: string, query: URLSearchParams) {
    const { rec } = this.readable(person, chatId);
    const limitText = query.get('limit');
    const limit = limitText === null ? MESSAGES_DEFAULT : Number(limitText);
    if (!Number.isFinite(limit) || limit < 1) invalid('limit', 'must be at least 1');
    const cap = Math.min(limit, MESSAGES_MAX);
    const sinceText = query.get('since');
    const view = this.view(person, rec);
    if (sinceText === null) {
      const tail = rec.messages.slice(-cap);
      return { chat: view, seq: rec.seq, since: null, complete: true, messages: tail, has_older: rec.messages.length > tail.length };
    }
    const since = Number(sinceText);
    const changed = rec.messages.filter((message) => message.seq_updated > since);
    if (changed.length > SYNC_MAX_CHANGES) return { chat: view, seq: rec.seq, since, complete: false, messages: [], has_older: false };
    return { chat: view, seq: rec.seq, since, complete: true, messages: changed, has_older: false };
  }

  private messages(person: Person, chatId: string, query: URLSearchParams) {
    const { rec } = this.readable(person, chatId);
    const limit = Math.min(Number(query.get('limit') ?? MESSAGES_DEFAULT) || MESSAGES_DEFAULT, MESSAGES_MAX);
    const all = rec.messages;
    const ways = ['before', 'after', 'around', 'pinned', 'item_id'].filter((way) => query.has(way));
    if (ways.length !== 1) invalid('query', 'exactly one of before, after, around, pinned and item_id is needed');
    if (query.has('before')) {
      const before = Number(query.get('before'));
      const older = all.filter((message) => message.seq_created < before);
      const page = older.slice(-limit);
      return { messages: page, has_older: older.length > page.length, has_newer: all.some((message) => message.seq_created >= before) };
    }
    if (query.has('after')) {
      const after = Number(query.get('after'));
      const newer = all.filter((message) => message.seq_created > after);
      const page = newer.slice(0, limit);
      return { messages: page, has_older: all.some((message) => message.seq_created <= after), has_newer: newer.length > page.length };
    }
    if (query.has('around')) {
      const at = all.findIndex((message) => message.id === query.get('around'));
      if (at < 0) return notFound();
      const from = Math.max(0, at - Math.floor(limit / 2));
      const to = Math.min(all.length, from + limit);
      return { messages: all.slice(from, to), has_older: from > 0, has_newer: to < all.length };
    }
    if (query.has('pinned')) return { messages: all.filter((message) => message.pinned !== null && !message.deleted), has_older: false, has_newer: false };
    const item = query.get('item_id');
    const quoting = all.filter((message) => message.parts.some((part) => !isForwarded(part) && part.type === 'item_ref' && part.item_id === item));
    return { messages: quoting, has_older: false, has_newer: false };
  }

  private parts(raw: unknown, path: string): ChatPart[] {
    if (!Array.isArray(raw) || raw.length === 0) invalid(path, 'a message has at least one part');
    const parts = raw as ChatPart[];
    if (parts.length > 32) invalid(path, 'a message has at most 32 parts');
    for (const part of parts) {
      if (isForwarded(part)) invalid(path, 'a copy is made by forwarding');
    }
    if (parts.every((part) => !isForwarded(part) && part.type === 'text' && part.text.trim() === '')) invalid(path, 'a message says something');
    return parts;
  }

  /** The people a message names: `@username` of somebody who can read the chat, and `@all`. */
  private mentioned(rec: Rec, parts: readonly ChatPart[], author: string): { users: UserRef[]; all: boolean } {
    const text = textOf(parts.filter((part) => !isForwarded(part) && part.type === 'text'));
    const users = new Map<string, UserRef>();
    let all = false;
    for (const match of text.matchAll(MENTION)) {
      const name = (match[2] ?? '').toLowerCase();
      if (name === 'all') {
        all = true;
        continue;
      }
      const person = this.person(name);
      if (person === undefined || name === author) continue;
      const readable = rec.kind === 'node' ? this.isMember(person, rec.orgId) : rec.members.includes(name);
      if (readable && users.size < 20) users.set(name, userRef(person.me));
    }
    return { users: [...users.values()], all };
  }

  private excerptOf(message: ChatMessage): NonNullable<ChatMessage['reply_to']> {
    const author = message.author;
    return {
      id: message.id,
      author,
      deleted: message.deleted,
      excerpt: Array.from(textOf(message.parts).split('\n')[0] ?? '').slice(0, REPLY_EXCERPT_CHARS).join(''),
    };
  }

  /** Writes a message into a chat and tells whoever has to know. `at` is for the tests that need a time of their own. */
  post(rec: Rec, username: string, parts: ChatPart[], replyTo: string | null, at: string = now(), accepting?: (message: ChatMessage) => void, origin?: {agent:string;thread:NodeRef}): ChatMessage {
    const { users, all } = this.mentioned(rec, parts, username);
    const replied = replyTo === null ? undefined : rec.messages.find((message) => message.id === replyTo);
    if (replyTo !== null && replied === undefined) invalid('reply_to', 'is not a message of this chat');
    rec.seq += 1;
    const message: ChatMessage = {
      mention_names: [],
      id: randomUUID(),
      chat_id: rec.id,
      kind: 'message',
      author: origin ?? this.ref(username),
      parts,
      reply_to: replied === undefined ? null : this.excerptOf(replied),
      mentions: users,
      mention_targets: [],
      asked_agents: [],
      mentions_all: origin === undefined && all,
      reactions: [],
      pinned: null,
      deleted: false,
      system: null,
      edited_at: null,
      created_at: at,
      seq_created: rec.seq,
      seq_updated: rec.seq,
    };
    accepting?.(message);
    rec.messages.push(message);
    rec.lastMessageSeq = rec.seq;
    rec.lastMessageAt = at;
    if (origin === undefined) {
      this.ensureState(username, rec, true);
      // An agent post never engages or reads on behalf of a person.
      const own = this.stateOf(username, rec.id);
      if (own !== null) this.states.set(`${username}:${rec.id}`, { ...own, last_read_seq: Math.max(own.last_read_seq, rec.seq) });
    }
    for (const user of message.mentions) {
      const state = this.ensureState(user.username, rec, true);
      this.states.set(`${user.username}:${rec.id}`, { ...state, mentions_unread: state.mentions_unread + 1 });
      this.tellUser(user.username, { what: 'chat_state', state: this.stateOf(user.username, rec.id) });
    }
    if (rec.kind !== 'node') {
      for (const member of rec.members) {
        const state = this.ensureState(member, rec);
        if (state.hidden) this.states.set(`${member}:${rec.id}`, { ...state, hidden: false });
        this.tellUser(member, { what: 'chat', chat_id: rec.id, seq: rec.seq, at });
      }
    }
    this.afterChange(rec, [message]);
    return message;
  }

  /** The scripted agent's already-guarded post uses the ordinary message and feed producer. */
  postAgent(chat: string, agent: string, thread: NodeRef, text: string): ChatMessage {
    return this.post(this.rec(chat),'',[{type:'text',text}],null,now(),undefined,{agent,thread});
  }

  private system(rec: Rec, event: 'created' | 'renamed' | 'member_added' | 'member_left' | 'member_removed', by: string, user: string | null, name: string | null): void {
    rec.seq += 1;
    rec.messages.push({
      id: randomUUID(),
      chat_id: rec.id,
      kind: 'system',
      author: null,
      parts: [],
      reply_to: null,
      mentions: [],
      mention_targets: [],
      asked_agents: [],
      mentions_all: false,
      reactions: [],
      pinned: null,
      deleted: false,
      system: { event, by: this.ref(by), user: user === null ? null : this.ref(user), name },
      edited_at: null,
      created_at: now(),
      seq_created: rec.seq,
      seq_updated: rec.seq,
    });
  }

  send(person: Person, chatId: string, body: Record<string, unknown>, accepting?: (message: ChatMessage) => void): Answer {
    const { rec } = this.writable(person, chatId);
    const clientId = asText(body.client_id);
    if (clientId === '') invalid('client_id', 'is needed');
    const key = `${person.me.username}:${clientId}`;
    const stored = rec.sent.get(key);
    if (stored !== undefined) {
      const message = rec.messages.find((one) => one.id === stored);
      if (message !== undefined) return { status: 200, body: { message, seq: rec.seq } };
    }
    const parts = this.parts(body.parts, 'parts');
    const replyTo = typeof body.reply_to === 'string' ? body.reply_to : null;
    const message = this.post(rec, person.me.username, parts, replyTo, now(), accepting);
    rec.sent.set(key, message.id);
    return { status: 201, body: { message, seq: rec.seq } };
  }

  private mutate(person: Person, chatId: string, messageId: string, rule: 'author' | 'sender' | 'author-or-moderator', change: (message: ChatMessage, rec: Rec) => ChatMessage | null): Answer {
    const { rec, standing } = this.writable(person, chatId);
    const at = rec.messages.findIndex((message) => message.id === messageId);
    const held = rec.messages[at];
    if (held?.kind !== 'message') return notFound();
    const own = held.author !== null && 'username' in held.author && held.author.username === person.me.username;
    if ((rule === 'author' && !own) || (rule === 'author-or-moderator' && !own && !standing.moderates)) {
      throw new Refusal(403, 'authz.denied', 'only the author of a message does that', { required: 'chat.interact', requestable: false, override: false, missing: [] });
    }
    const next = change(held, rec);
    if (next === null) return { status: 200, body: held };
    rec.seq += 1;
    const changed: ChatMessage = { ...next, seq_updated: rec.seq };
    rec.messages[at] = changed;
    this.afterChange(rec, [changed]);
    return { status: 200, body: changed };
  }

  edit(person: Person, chatId: string, messageId: string, body: Record<string, unknown>, accepting?: (message: ChatMessage) => void): Answer {
    const parts = this.parts(body.parts, 'parts');
    return this.mutate(person, chatId, messageId, 'author', (message, rec) => {
      if (message.deleted) invalid('message', 'was deleted');
      const { users, all } = this.mentioned(rec, parts, person.me.username);
      const next = { ...message, parts, mentions: users, mentions_all: all, edited_at: now() };
      accepting?.(next); return next;
    });
  }

  private remove(person: Person, chatId: string, messageId: string): Answer {
    return this.mutate(person, chatId, messageId, 'author-or-moderator', (message) =>
      message.deleted ? null : { ...message, deleted: true, parts: [], reactions: [], mentions: [], mention_names:[],mention_targets:[],asked_agents:[],mentions_all: false, pinned: null },
    );
  }

  private react(person: Person, chatId: string, messageId: string, emoji: string, on: boolean): Answer {
    if (emoji === '' || Array.from(emoji).length > 16 || /\s/.test(emoji)) invalid('emoji', 'is one to sixteen characters without white space');
    return this.mutate(person, chatId, messageId, 'sender', (message) => {
      if (message.deleted) invalid('message', 'was deleted');
      const me = person.me.id;
      const held = message.reactions.find((one) => one.emoji === emoji);
      if (on) {
        if (held?.users.includes(me) === true) return null;
        if (held === undefined && message.reactions.length >= REACTION_KINDS_MAX) invalid('emoji', 'a message has at most 20 kinds of reaction');
        const reactions = held === undefined ? [...message.reactions, { emoji, users: [me] }] : message.reactions.map((one) => (one.emoji === emoji ? { ...one, users: [...one.users, me] } : one));
        return { ...message, reactions };
      }
      if (held?.users.includes(me) !== true) return null;
      const reactions = message.reactions
        .map((one) => (one.emoji === emoji ? { ...one, users: one.users.filter((user) => user !== me) } : one))
        .filter((one) => one.users.length > 0);
      return { ...message, reactions };
    });
  }

  private pin(person: Person, chatId: string, messageId: string, on: boolean): Answer {
    return this.mutate(person, chatId, messageId, 'sender', (message) => {
      if (message.deleted) invalid('message', 'was deleted');
      if (on === (message.pinned !== null)) return null;
      return { ...message, pinned: on ? { at: now(), by: this.ref(person.me.username) } : null };
    });
  }

  // ---------------------------------------------------------------- reading, the bell, drafts

  private read(person: Person, chatId: string, body: Record<string, unknown>): Answer {
    const { rec, standing } = this.readable(person, chatId);
    if (standing.inspects) throw new Refusal(403, 'authz.denied', 'they have no read position there', { required: 'chat.interact', requestable: false, override: false, missing: [] });
    const seq = Number(body.seq);
    if (!Number.isFinite(seq) || seq < 0) invalid('seq', 'must not be negative');
    const username = person.me.username;
    const state = this.ensureState(username, rec);
    this.states.set(`${username}:${rec.id}`, { ...state, last_read_seq: Math.max(state.last_read_seq, Math.min(seq, rec.seq)), mentions_unread: 0 });
    this.tellUser(username, { what: 'chat_state', state: this.stateOf(username, rec.id) });
    return { status: 204 };
  }

  private readAll(person: Person, orgId: string): Answer {
    if (!this.world.trees.has(orgId)) return notFound();
    const username = person.me.username;
    for (const [key, state] of this.states) {
      if (!key.startsWith(`${username}:`) || state.effective_level === 'off') continue;
      const rec = this.records.get(state.chat_id);
      if (rec?.orgId !== orgId) continue;
      this.states.set(key, { ...state, last_read_seq: rec.seq, mentions_unread: 0 });
      this.tellUser(username, { what: 'chat_state', state: this.stateOf(username, rec.id) });
    }
    return { status: 204 };
  }

  private setState(person: Person, chatId: string, body: Record<string, unknown>): Answer {
    const { rec, standing } = this.readable(person, chatId);
    if (standing.inspects) throw new Refusal(403, 'authz.denied', 'they have no state there', { required: 'chat.interact', requestable: false, override: false, missing: [] });
    const username = person.me.username;
    const held = this.ensureState(username, rec);
    const next: ChatState = { ...held };
    if ('level' in body) {
      const level = body.level;
      if (level !== null && level !== 'all' && level !== 'mentions' && level !== 'off') invalid('level', 'is all, mentions, off or null');
      next.level = level;
    }
    if (typeof body.hidden === 'boolean') next.hidden = body.hidden;
    next.effective_level = next.level ?? (next.engaged || (rec.kind !== 'node' && rec.members.includes(username)) ? 'all' : 'off');
    this.states.set(`${username}:${rec.id}`, next);
    this.tellUser(username, { what: 'chat_state', state: next });
    return { status: 200, body: next };
  }

  private saveDraft(person: Person, chatId: string, body: Record<string, unknown>): Answer {
    this.writable(person, chatId);
    const draft: ChatDraft = {
      mention_names: [],
      parts: Array.isArray(body.parts) ? (body.parts as ChatPart[]) : [],
      reply_to: typeof body.reply_to === 'string' ? body.reply_to : null,
      updated_at: now(),
    };
    this.drafts.set(`${person.me.username}:${chatId}`, draft);
    this.tellUser(person.me.username, { what: 'chat_draft', chat_id: chatId, draft });
    return { status: 200, body: draft };
  }

  // ---------------------------------------------------------------- forwarding

  private forward(person: Person, body: Record<string, unknown>): Answer {
    const from = (typeof body.from === 'object' && body.from !== null ? body.from : {}) as Record<string, unknown>;
    const to = Array.isArray(body.to) ? (body.to as Record<string, unknown>[]) : [];
    // What is copied: messages of a chat, or items of a thread (read behind `thread.view`, in the order they were picked).
    let source: Rec;
    let copiesOf: (() => ChatPart[]) | null = null;
    if (typeof from.chat_id === 'string') {
      const origin = this.readable(person, from.chat_id).rec;
      source = origin;
      const messageIds = Array.isArray(from.message_ids) ? (from.message_ids as string[]) : [];
      if (messageIds.length === 0 || messageIds.length > FORWARD_MAX_MESSAGES) invalid('from.message_ids', 'takes one to twenty messages');
      const picked = messageIds.map((id) => origin.messages.find((message) => message.id === id && message.kind === 'message' && !message.deleted));
      if (picked.some((message) => message === undefined)) invalid('from.message_ids', 'names a message that is not there');
      copiesOf = () => copiesOfMessages({id:origin.id,kind:origin.kind,name:origin.name,node:this.nodeRef(origin),org_id:origin.orgId},picked as ChatMessage[]);
    } else if (typeof from.thread_id === 'string') {
      const threadId = from.thread_id;
      this.world.places.need(person, threadId, 'thread.view');
      const row = this.world.places.rowOf(threadId);
      if (row?.kind !== 'thread') return notFound();
      source = this.rec(row.chat.id);
      const itemIds = Array.isArray(from.item_ids) ? (from.item_ids as string[]) : [];
      if (itemIds.length === 0 || itemIds.length > FORWARD_MAX_ITEMS) invalid('from.item_ids', 'takes one to two hundred items');
      const picked = itemIds.map((id) => this.world.threads.itemOf(threadId, id));
      if (picked.some((item) => item === undefined)) invalid('from.item_ids', 'names an item that is not there');
      copiesOf = () =>
        (picked as Item[]).map((item) => ({
          type: 'forwarded' as const,
          at: item.created_at,
          attachments: [],
          author: item.author,
          from: { kind: 'thread', thread: { id: threadId, kind: 'thread', name: row.name, redacted: false } },
          images: [],
          source_id: item.id,
          text: wordsOfItem(item),
        })) as unknown as ChatPart[];
    } else {
      invalid('from', 'names a chat or a thread');
    }
    if (to.length === 0 || to.length > FORWARD_MAX_DESTINATIONS) invalid('to', 'takes one to ten destinations');
    const stable = `${person.me.id}:${String(body.client_id)}`;
    const previous = this.forwards.get(stable);
    if (previous !== undefined) return {...previous,status:200};
    const comment = typeof body.comment === 'string' && body.comment.trim() !== '' ? body.comment.trim() : null;
    const copies = copiesOf();
    const content: ContentPart[] = [...(comment === null ? [] : [{type:'text' as const,text:comment}]),...copies.filter((part) => part.type === 'text' || part.type === 'forwarded')];
    const threadIds = [...new Set(to.flatMap((destination) => typeof destination.thread_id === 'string' ? [destination.thread_id] : []))].sort();
    const prepared = threadIds.map((id) => {
      try { return this.world.threads.prepareInput(person,id,{content,delivery:'queue',client_id:body.client_id}); }
      catch (error) { if (error instanceof Refusal) error.answer.body = {...error.answer.body as object,object:{thread_id:id}}; throw error; }
    });
    // Validate every people/chat destination before a new direct chat or input is written.
    for (const destination of to) {
      if (typeof destination.chat_id === 'string') this.writable(person,destination.chat_id);
      else if (typeof destination.user_id === 'string') {
        const other = [...this.world.people.values()].find((one) => one.me.id === destination.user_id);
        if (other === undefined || !this.isMember(other,source.orgId)) throw new Refusal(404,'not_found','no such person',{object:destination});
      } else if (typeof destination.thread_id !== 'string') invalid('to','names a chat, person or thread');
    }
    // Every destination is checked before anything is written.
    const targets = new Map<string, Rec>();
    for (const destination of to) {
      if (typeof destination.thread_id === 'string') continue;
      if (typeof destination.chat_id === 'string') {
        let target: Rec;
        try {
          target = this.writable(person, destination.chat_id).rec;
        } catch (error) {
          if (error instanceof Refusal) {
            error.answer.body = { ...(error.answer.body as object), object: destination };
          }
          throw error;
        }
        targets.set(target.id, target);
      } else if (typeof destination.user_id === 'string') {
        const other = [...this.world.people.values()].find((one) => one.me.id === destination.user_id);
        if (other === undefined || !this.isMember(other, source.orgId)) {
          throw new Refusal(404, 'not_found', 'no such person', { object: destination });
        }
        let direct = [...this.records.values()].find(
          (candidate) => candidate.kind === 'direct' && candidate.orgId === source.orgId && candidate.members.includes(person.me.username) && candidate.members.includes(other.me.username),
        );
        direct ??= this.newRec('direct', source.orgId, [person.me.username, other.me.username], null, person.me.username);
        this.ensureState(person.me.username, direct);
        this.ensureState(other.me.username, direct);
        targets.set(direct.id, direct);
      } else {
        throw new Refusal(409, 'capability.unsupported', 'forwarding to an agent\'s thread is not served', { capability: 'forward.thread', object: destination });
      }
    }
    const written = [...targets.values()]
      .sort((a, b) => a.id.localeCompare(b.id))
      .map((target) => ({ chat_id: target.id, message: this.post(target, person.me.username, [...(comment === null ? [] : [{ type: 'text' as const, text: comment }]), ...copies], null) }));
    const order = to.map((destination) => {
      const id = typeof destination.chat_id === 'string' ? destination.chat_id : [...targets.values()].find((target) => target.kind === 'direct' && target.members.includes(this.userNameOf(destination.user_id)))?.id;
      return written.find((one) => one.chat_id === id);
    });
    const threads = prepared.map((input) => ({thread_id:input.thread_id,input:this.world.threads.acceptInput(person,input).input}));
    const result: Answer = { status: 201, body: { chats: order.filter((one) => one !== undefined), threads } };
    this.forwards.set(stable,result); return result;
  }

  private userNameOf(id: unknown): string {
    return [...this.world.people.values()].find((one) => one.me.id === id)?.me.username ?? '';
  }

  // ---------------------------------------------------------------- the live connection

  /** What changed in a chat, told to its followers and, for a node's chat, to the tree. */
  private afterChange(rec: Rec, rows: ChatMessage[] = []): void {
    if (rec.kind === 'node' && rec.nodeId !== null && rows.some((row) => row.seq_created === rec.seq && row.kind === 'message')) {
      this.world.changeTree(rec.orgId, [{ id: rec.nodeId, chat: { id: rec.id, seq: rec.lastMessageSeq } }]);
    }
    const followers = this.followers.get(rec.id) ?? [];
    for (const follower of followers) {
      const person = this.person(follower.username);
      if (person === undefined) continue;
      const prev = follower.lastSent;
      follower.lastSent = rec.seq;
      if (this.lose.delete(rec.id)) continue;
      follower.subscriber.send({
        op: 'sync',
        id: follower.id,
        feed: `chat:${rec.id}`,
        prev,
        seq: rec.seq,
        data: { chat: this.view(person, rec), messages: rows },
      });
    }
  }

  /** An event for the person's own feed: the world's, which every part tells a person's events to. */
  private tellUser(username: string, data: Record<string, unknown>): void {
    this.world.users.tell(username, data);
  }

  /** The frames of the live connection about a chat's feed: a subscription, the end of one, whether the chat is on screen, and somebody typing. */
  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const feed = typeof frame.feed === 'string' ? frame.feed : '';
    switch (frame.op) {
      case 'sub':
        return this.subscribe(subscriber, person, Number(frame.id), feed, frame.since);
      case 'unsub': {
        const chatId = this.unsubscribe(subscriber, Number(frame.id));
        if (chatId === null) return false;
        subscriber.send({ op: 'closed', id: Number(frame.id), feed: `chat:${chatId}`, reason: 'unsubscribed' });
        return true;
      }
      case 'visible':
        if (!feed.startsWith('chat:')) return false;
        this.visible(subscriber, feed, frame.visible === true);
        return true;
      case 'typing':
        if (!feed.startsWith('chat:')) return false;
        this.typing(subscriber, feed);
        return true;
      default:
        return false;
    }
  }

  /** Takes a subscription to a chat's feed. Answers whether the feed is one. */
  private subscribe(subscriber: Subscriber, person: Person | undefined, id: number, feed: string, since: unknown): boolean {
    const chatId = /^chat:(.+)$/.exec(feed)?.[1];
    if (chatId === undefined) return false;
    if (person === undefined) {
      subscriber.send({ op: 'error', id, code: 'forbidden', detail: feed });
      return true;
    }
    let held: { rec: Rec; standing: Standing };
    try {
      held = this.readable(person, chatId);
    } catch (error) {
      const gone = error instanceof Refusal && error.answer.status === 404;
      subscriber.send({ op: 'error', id, code: gone ? 'not_found' : 'forbidden', detail: feed });
      return true;
    }
    const { rec, standing } = held;
    const follower: Follower = { subscriber, id, username: person.me.username, inspecting: standing.inspects, visible: false, since: now(), lastSent: rec.seq };
    const list = this.followers.get(rec.id) ?? [];
    list.push(follower);
    this.followers.set(rec.id, list);
    const from = typeof since === 'number' ? since : null;
    const reload = from !== null && from > rec.seq;
    subscriber.send({ op: 'subscribed', id, feed, seq: rec.seq, mode: reload ? 'reload' : 'delta' });
    if (!reload && from !== null && from < rec.seq) {
      const rows = rec.messages.filter((message) => message.seq_updated > from);
      subscriber.send({ op: 'sync', id, feed, prev: from, seq: rec.seq, data: { chat: this.view(person, rec), messages: rows } });
    }
    return true;
  }

  /** Ends a subscription; the chat it was to, or `null` when it was to none. */
  private unsubscribe(subscriber: Subscriber, id: number): string | null {
    let found: string | null = null;
    for (const [chatId, list] of this.followers) {
      const left = list.filter((follower) => !(follower.subscriber === subscriber && follower.id === id));
      if (left.length !== list.length) {
        found = chatId;
        this.followers.set(chatId, left);
        this.sharePresence(chatId);
      }
    }
    return found;
  }

  /** A socket ended. */
  gone(subscriber: Subscriber): void {
    for (const [chatId, list] of this.followers) {
      const left = list.filter((follower) => follower.subscriber !== subscriber);
      if (left.length !== list.length) {
        this.followers.set(chatId, left);
        this.sharePresence(chatId);
      }
    }
  }

  visible(subscriber: Subscriber, feed: string, visible: boolean): void {
    const chatId = /^chat:(.+)$/.exec(feed)?.[1];
    if (chatId === undefined) return;
    const follower = (this.followers.get(chatId) ?? []).find((one) => one.subscriber === subscriber);
    if (follower === undefined || follower.inspecting) return;
    follower.visible = visible;
    this.sharePresence(chatId);
  }

  private sharePresence(chatId: string): void {
    const list = this.followers.get(chatId) ?? [];
    const looking = new Map<string, { user: UserRef; since: string }>();
    for (const follower of list) {
      if (!follower.visible || follower.inspecting || looking.has(follower.username)) continue;
      looking.set(follower.username, { user: this.ref(follower.username), since: follower.since });
    }
    for (const follower of list) {
      follower.subscriber.send({ op: 'presence', id: follower.id, feed: `chat:${chatId}`, users: [...looking.values()] });
    }
  }

  /** Somebody types: the others who follow the chat are told, one frame in three seconds. */
  typing(subscriber: Subscriber, feed: string, at: number = Date.now()): void {
    const chatId = /^chat:(.+)$/.exec(feed)?.[1];
    if (chatId === undefined) return;
    const sender = (this.followers.get(chatId) ?? []).find((one) => one.subscriber === subscriber);
    if (sender === undefined || sender.inspecting) return;
    const key = `${chatId}:${sender.username}`;
    const before = this.typingAt.get(key);
    if (before !== undefined && at - before < 3_000) return;
    this.typingAt.set(key, at);
    this.typedBy(chatId, sender.username);
  }

  /** Tells the followers of a chat that somebody is typing, except that person. */
  typedBy(chatId: string, username: string): void {
    for (const follower of this.followers.get(chatId) ?? []) {
      if (follower.username === username) continue;
      follower.subscriber.send({ op: 'event', id: follower.id, feed: `chat:${chatId}`, data: { what: 'typing', user: this.ref(username) } });
    }
  }

  /** The person is no longer let into the chat: every subscription of theirs to it ends. */
  private closeFor(rec: Rec, username: string): void {
    const list = this.followers.get(rec.id) ?? [];
    for (const follower of list.filter((one) => one.username === username)) {
      follower.subscriber.send({ op: 'closed', id: follower.id, feed: `chat:${rec.id}`, reason: 'revoked' });
    }
    this.followers.set(rec.id, list.filter((one) => one.username !== username));
    this.sharePresence(rec.id);
  }

  // ---------------------------------------------------------------- the requests only the double has

  control(path: string, body: Record<string, unknown>): Answer {
    switch (path) {
      case '/seed':
        return { status: 200, body: this.seed() };
      case '/say': {
        // Somebody writes in a chat, as if from another browser. `node` names a node by its name, `chat` a chat by its id.
        const rec = this.recFor(body);
        const username = asText(body.as);
        if (this.person(username) === undefined) return { status: 400, body: { detail: `nobody is called ${username}` } };
        const parts = Array.isArray(body.parts) ? (body.parts as ChatPart[]) : [{ type: 'text' as const, text: asText(body.text) }];
        if (rec.kind !== 'node' && !rec.members.includes(username)) rec.members.push(username);
        const message = this.post(rec, username, parts, typeof body.reply_to === 'string' ? body.reply_to : null, typeof body.at === 'string' ? body.at : now());
        return { status: 200, body: { chat: rec.id, message } };
      }
      case '/typing': {
        const rec = this.recFor(body);
        this.typedBy(rec.id, asText(body.as));
        return { status: 204 };
      }
      case '/lose': {
        this.lose.add(this.recFor(body).id);
        return { status: 204 };
      }
      case '/chat': {
        // A direct or group chat among people, made without the page: `{ org, members: [username], name?, guests?: [username] }`.
        const orgId = asText(body.org);
        const members = Array.isArray(body.members) ? (body.members as string[]) : [];
        const rec = this.newRec(members.length === 2 && body.name === undefined ? 'direct' : 'group', orgId, members, typeof body.name === 'string' ? body.name : null, members[0] ?? '');
        for (const guest of Array.isArray(body.guests) ? (body.guests as string[]) : []) {
          rec.guests.add(guest);
          if (!rec.members.includes(guest)) rec.members.push(guest);
        }
        for (const member of rec.members) this.ensureState(member, rec);
        if (rec.kind === 'group') this.system(rec, 'created', members[0] ?? '', null, rec.name);
        this.afterChange(rec);
        return { status: 200, body: { chat: rec.id } };
      }
      case '/switch': {
        // Chats off or on for a node.
        const rec = this.recFor(body);
        if (body.off === true) this.switchedOff.add(rec.id);
        else this.switchedOff.delete(rec.id);
        return { status: 204 };
      }
      case '/item': {
        // An item that a chat message may quote, made in a thread of the one world as if the agent or a person had put it there: `{ thread, text, author?, kind? }`.
        const threadId = asText(body.thread);
        const itemId = randomUUID();
        const text = asText(body.text);
        const author = asText(body.author) === '' ? null : this.ref(asText(body.author));
        const item = body.kind === 'agent_message' ? agentMessage(itemId, text) : userMessage(itemId, text, { author: author ?? this.ref('ada') });
        this.world.threads.emit(threadId, { items: [item] });
        return { status: 200, body: { id: itemId } };
      }
      case '/no-timeline': {
        // Somebody who may not read any thread, and so no item of one: what a quote of an item then shows them is the fact of it.
        const username = asText(body.as);
        this.world.places.denied.set(username, new Set([...(this.world.places.denied.get(username) ?? []), 'thread.view']));
        return { status: 204 };
      }
      case '/locale': {
        // The language a person's profile says, as it is after they chose it on another device.
        const person = this.person(asText(body.as));
        if (person === undefined) return { status: 400, body: { detail: `nobody is called ${asText(body.as)}` } };
        person.me = { ...person.me, locale: body.locale === 'zh-CN' ? 'zh-CN' : 'en' };
        return { status: 204 };
      }
      case '/guest': {
        // A person from outside the org who is a guest of a group, and nothing else of the tree.
        const username = asText(body.username) === '' ? 'gus' : asText(body.username);
        if (this.person(username) === undefined) {
          // Other fixture families already occupy the numbered identities; a guest is a new person.
          this.world.addPerson(makeMe(10, username, 'Gus Guest', { id: randomUUID(), personal_org_id: randomUUID() }), 'a guest has a long password');
        }
        return { status: 200, body: { username } };
      }
      default:
        return { status: 404, body: { detail: `no such control: /chats${path}` } };
    }
  }

  private recFor(body: Record<string, unknown>): Rec {
    if (typeof body.chat === 'string') return this.rec(body.chat);
    const name = asText(body.node);
    const row = this.nodeRows().find((one) => one.row.name === name && !one.row.deleted)?.row;
    if (row === undefined) return notFound();
    return this.rec(row.chat.id);
  }

  /** A conversation to look at: messages by several people in the chat of Fix login, with a reply, an edit, a reaction and a pin. */
  seed(): { chat: string } {
    const rec = this.recFor({ node: 'Fix login' });
    const ago = (minutes: number): string => new Date(Date.now() - minutes * 60_000).toISOString();
    const first = this.post(rec, 'dora', [{ type: 'text', text: 'Has anybody seen the session cookie expire early on the staging box?' }], null, ago(190));
    this.post(rec, 'grace', [{ type: 'text', text: 'Yes, twice this morning. I think the refresh runs after the redirect.' }], first.id, ago(185));
    this.post(rec, 'grace', [{ type: 'text', text: 'I will write up what I saw in the topic.' }], null, ago(184));
    const third = this.post(rec, 'linus', [{ type: 'text', text: 'Please take a look at **the handler** first: `src/login.ts` loses the `next` parameter.' }], null, ago(60));
    this.post(rec, 'ada', [{ type: 'text', text: 'Thanks @linus, that fits what the agent found.' }], third.id, ago(58));
    return { chat: rec.id };
  }
}
