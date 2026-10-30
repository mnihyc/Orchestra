/** What the fake API says of places: nodes with what their caller holds on them, their topics, tags, Activity, members and invitations, and the acts that make, change, archive, move and delete them.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: the same
 * shapes, the same refusals with what is missing and whom to ask, the same revisions and the same
 * way of reading a list a page at a time. The structure of the tree stays in `World.trees`, which the
 * live feed and the left bar read; what a node says beyond it (description, revision, owner, slug,
 * tags, who did what) is kept here, and every act changes both.
 */
import { createHash, randomUUID } from 'node:crypto';

import type {
  Access,
  ActivityEntry,
  Favorite,
  Invitation,
  Member,
  NotificationException,
  Node,
  NodeArchivePlan,
  NodeArchiveResult,
  NodeKind,
  NodeMovePlan,
  NodeRef,
  Org,
  PlanBlocker,
  PreviewPerson,
  Project,
  Role,
  RoleHeld,
  Tag,
  Topic,
  TreeNode,
  UserRef,
} from '../../src/generated/api-types.ts';
import { MADE_AT, id as fixtureId, userRef } from './fixtures.ts';
import { display } from './thread-display.ts';
import { type Answer, type Person, Refusal, type Subscriber, type World } from './world.ts';

/** The most a page of a list holds unless it asks for fewer, and the least. */
const PAGE_DEFAULT = 50;
const PAGE_MAX = 200;
/** Stored confirmations have the same fifteen-minute lifetime as the real place plans. */
const REVIEW_LIFE_MS = 15 * 60_000;
/** The shipped soft-delete recovery window in this browser fixture. */
const DELETE_KEEP_MS = 30 * 86_400_000;
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** `[limits] project_depth`: how many projects one inside another a path holds. */
export const PROJECT_DEPTH = 8;

/** What moving something into a place changes for people, as the plan says: the fake gives a few places an effect and the rest none. */
interface MoveEffect {
  gaining: { username: string; pages: number }[];
  losing: { username: string; pages: number }[];
  roles: { gaining: number; losing: number };
  settings: { key: string; from: unknown; to: unknown; lockedBy: string | null }[];
}

/** What a node says that its row of the tree does not. */
interface Meta {
  description: string;
  /** The document named as this place's README, where one is named. */
  readmeDocumentId?: string | null;
  revision: number;
  owner: string;
  createdAt: string;
  updatedAt: string;
  /** An org's short name: one segment of its address. */
  slug?: string;
  /** Assigned tags of any place, by identifier. */
  tagIds: string[];
  /** The number of a topic in its project. */
  seq: number;
  /** Whether it was archived itself, and not through a place above it. */
  archivedItself: boolean;
  deletedAt?: string;
  deletedBy?: string;
  /** Whether it was deleted itself, and not with a place above it. */
  deletedItself?: boolean;
  doneAt?: string;
}

interface Line {
  at: string;
  actor: string;
  action: string;
  title: string;
  node: string;
  /** The node the line is about and the one it lies directly in, so that both places list it. */
  parent: string | null;
  kind: ActivityEntry['kind'];
  thread: string | null;
}

interface InviteRecord {
  invitation: Invitation;
  token: string;
}

/** What each role holds on a node of each kind, as the API lists it: the kind's own permissions, never the role's name. */
function permissionsOf(kind: NodeKind, role: Role | null, system: boolean): string[] {
  if (role === null && !system) return [];
  const own = (verbs: string[]) => verbs.map((verb) => `${kind}.${verb}`);
  const crosses = (verbs: string[]) => verbs.filter((verb) => (kind === 'org' ? true : verb !== 'members.manage'));
  // Chats are open to everybody on a node as shipped; an admin may also delete what others wrote, and read the chats among an org's people.
  // Taking a chat or a document away as a file is held by whoever reads it, as shipped; a thread's row is its own.
  const base = [...own(['view']), 'usage.summary', 'chat.view', 'chat.interact', 'chat.export', 'doc.export', ...(kind === 'thread' ? ['thread.export'] : [])];
  switch (system ? 'admin' : role) {
    case 'viewer':
    case 'interactor':
      return base;
    case 'developer':
      return [...base, ...(kind === 'org' ? ['org.interact'] : kind === 'project' ? ['project.interact'] : kind === 'topic' ? ['topic.interact', 'topic.manage', 'topic.archive'] : ['thread.interact', 'thread.manage', 'thread.archive'])];
    default:
      return [...base, ...own(['interact', 'manage', 'archive']), ...crosses(['members.manage']), 'usage.view', 'usage.manage', 'usage.bypass', 'access.manage', 'access.own', 'chat.manage', 'settings.manage', 'audit.view', ...(kind === 'org' ? ['directs.view'] : [])];
  }
}

/** What a role holds on the documents written beside a node: everybody who reads makes and comments, those who interact write, developers manage. */
function documentRows(role: Role | null, system: boolean): string[] {
  if (role === null && !system) return [];
  const rows = ['doc.view', 'doc.create', 'doc.comment'];
  const rank = system ? 3 : ROLES.indexOf(role ?? 'viewer');
  if (rank >= 1) rows.push('doc.edit');
  if (rank >= 2) rows.push('doc.manage');
  return rows;
}

const ROLES: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];

/** A member's role in a root org: where it applies, since when, and the revision a change must have seen. */
interface RoleOf {
  role: Role;
  domain: string;
  since: string;
  roleId: string;
  revision: number;
}

/** What a request said in a member that is to be text: the text, and nothing where it said none. */
const asText = (value: unknown): string => (typeof value === 'string' ? value : '');

/** The most numbered short names tried when one is offered in place of a taken one, as the real system tries. */
const SUGGESTIONS_TRIED = 99;
/** The most characters of a short name. */
const SLUG_MAX_CHARS = 32;

/**
 * The refusal of a short name another org of the same place holds, as the real system answers it: the org that holds it
 * where the person sees the place (`holder`, none at the root), and the first free short name of the same stem.
 */
function slugTaken(slug: string, holder: { id: string; name: string } | null, taken: readonly string[]): Refusal {
  let suggestion: string | null = null;
  for (let number = 2; number <= SUGGESTIONS_TRIED && suggestion === null; number += 1) {
    const tail = `-${number}`;
    const offered = `${slug.slice(0, SLUG_MAX_CHARS - tail.length).replace(/[-_]+$/, '')}${tail}`;
    if (!taken.includes(offered)) suggestion = offered;
  }
  const detail = holder === null ? `the address ${slug} is taken` : `${holder.name} holds the short name ${slug} here already`;
  return new Refusal(409, 'conflict.exists', detail, {
    errors: [{ path: 'slug', message: suggestion === null ? detail : `${detail}; ${suggestion} is free` }],
    ...(holder === null ? {} : { existing_id: holder.id, existing: { id: holder.id, name: holder.name, deleted: false } }),
    ...(suggestion === null ? {} : { suggestion }),
  });
}

export class Places {
  readonly world: World;
  meta = new Map<string, Meta>();
  tags = new Map<string, Tag>();
  lines: Line[] = [];
  /** The role each person holds at their domain, by root org and username. */
  roles = new Map<string, Map<string, RoleOf>>();
  invitations = new Map<string, InviteRecord[]>();
  favorites = new Map<string, string[]>();
  subscriptions = new Map<string, 'all' | 'important' | 'off'>();
  /** Permissions a test took away from somebody, by username: what a stale page would still believe they hold. */
  denied = new Map<string, Set<string>>();
  /** Permissions a test gave somebody on one node (`permission@node`), by username: what a page that names them there would give. */
  granted = new Map<string, Set<string>>();
  /** Who reads the tree of an org through which live connection, so that each is sent the rows they may see. */
  private readers = new Map<Subscriber, string>();
  /** The mail the fake API cannot send, for the personal links it makes. */
  mailWorks = false;
  private clock = 0;
  private archiveReviews = new Map<string, { user: string; plan: NodeArchivePlan }>();
  private moveReviews = new Map<string, { user: string; plan: NodeMovePlan }>();
  private deleteReviews = new Map<string, { user: string; node: string; stamp: string; expires: number }>();

  constructor(world: World) {
    this.world = world;
  }

  // ---------------------------------------------------------------- reading the structure

  private rows(): Map<string, TreeNode> {
    const all = new Map<string, TreeNode>();
    for (const tree of this.world.trees.values()) for (const [key, row] of tree.nodes) all.set(key, row);
    return all;
  }

  private row(id: string): TreeNode | undefined {
    for (const tree of this.world.trees.values()) {
      const found = tree.nodes.get(id);
      if (found !== undefined) return found;
    }
    return undefined;
  }

  /** The row of the tree that holds a node, for what is not about places. */
  rowOf(id: string): TreeNode | undefined {
    return this.row(id);
  }

  /** A row that has to be there, since the request that is being answered has been shown to be about it. */
  private must(id: string): TreeNode {
    const found = this.row(id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such place');
    return found;
  }

  /** The root org whose tree holds a node. */
  rootOf(id: string): string | null {
    for (const [root, tree] of this.world.trees) if (tree.nodes.has(id)) return root;
    return null;
  }

  /** The nodes from the root down to a node, itself last. */
  pathTo(id: string): TreeNode[] {
    const path: TreeNode[] = [];
    let at = this.row(id);
    for (let steps = 0; at !== undefined && steps < 64; steps += 1) {
      path.unshift(at);
      at = at.parent_id === null ? undefined : this.row(at.parent_id);
    }
    return path;
  }

  private children(id: string, includeDeleted = false): TreeNode[] {
    return [...this.rows().values()]
      .filter((row) => row.parent_id === id && (includeDeleted || !row.deleted))
      .sort((one, other) => one.tree_seq - other.tree_seq);
  }

  /** Descendants in the shared tree, for the fake world's place-level readers. */
  descendants(id: string): TreeNode[] {
    const found: TreeNode[] = [];
    const pending = this.children(id, true);
    for (let row = pending.pop(); row !== undefined; row = pending.pop()) {
      found.push(row);
      pending.push(...this.children(row.id, true));
    }
    return found;
  }

  metaOf(id: string): Meta {
    let held = this.meta.get(id);
    if (held === undefined) {
      held = { description: '', revision: 1, owner: 'ada', createdAt: MADE_AT, updatedAt: MADE_AT, tagIds: [], seq: 0, archivedItself: false };
      this.meta.set(id, held);
    }
    return held;
  }

  private person(username: string): Person | undefined {
    return this.world.people.get(username);
  }

  private ref(row: TreeNode): NodeRef {
    return { id: row.id, kind: row.kind, name: row.name, redacted: false, ...(row.kind === 'thread' && row.thread?.display !== undefined ? { thread_display: row.thread.display } : {}) };
  }

  /** Whether a person may see a node at all: they hold its View. */
  visibleTo(person: Person, row: TreeNode): boolean {
    return this.access(person, row.id).permissions.includes(`${row.kind}.view`);
  }

  /** A place on the way to something, as its reader may know it: one they may not see is named by its kind alone. */
  private refFor(person: Person, row: TreeNode): NodeRef {
    return this.visibleTo(person, row) ? this.ref(row) : { id: row.id, kind: row.kind, name: null, redacted: true };
  }

  /** Native home ancestry uses the same redacted references as the node page's path. */
  pathFor(person: Person, nodeId: string): NodeRef[] {
    return this.pathTo(nodeId).map((row) => this.refFor(person, row));
  }

  /** How many documents are written beside each node and follow its documents rows; one that was made private is not counted. */
  private documentCounts(): Map<string, number> {
    const counts = new Map<string, number>();
    for (const doc of this.world.documents.docs.values()) {
      if (doc.deleted || doc.page !== 'follows') continue;
      counts.set(doc.nodeId, (counts.get(doc.nodeId) ?? 0) + 1);
    }
    return counts;
  }

  /**
   * Rows of a tree as one person is given them: a row they may not see is sent as gone and says nothing else, and a row
   * says how many documents beside it they may read. `since` is the sequence the person holds, `null` for the whole tree:
   * in a whole tree what they may not see is left out.
   */
  rowsFor(person: Person, rows: readonly TreeNode[], since: number | null): TreeNode[] {
    const counts = this.documentCounts();
    const waits = this.world.notifications.treeWaits(person, new Set(rows.filter((row) => row.kind === 'thread').map((row) => row.id)));
    return rows.flatMap((row): TreeNode[] => {
      const access = row.deleted ? null : this.access(person, row.id);
      const held = access?.permissions ?? [];
      if (held.includes(`${row.kind}.view`)) return [{ ...row, participating: access?.participating === true, docs: held.includes('doc.view') ? (counts.get(row.id) ?? 0) : 0,
        thread: row.thread === null ? null : { ...row.thread, waiting: waits.get(row.id) ?? null } }];
      if (since === null) return [];
      return [{ ...row, name: '', address: null, color: null, docs: 0, deleted: true, restricted: false, thread: null, topic: null, chat: { id: row.chat.id, seq: 0 } }];
    });
  }

  /** The person who reads a tree through a live connection, where the connection said so. */
  readerOf(subscriber: Subscriber): Person | undefined {
    const username = this.readers.get(subscriber);
    return username === undefined ? undefined : this.world.people.get(username);
  }

  /** A frame of the live connection: none is the places' own, but one that follows the tree of an org says who reads it there. */
  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    if (frame.op === 'sub' && typeof frame.feed === 'string' && frame.feed.startsWith('org:') && person !== undefined) this.readers.set(subscriber, person.me.username);
    return false;
  }

  gone(subscriber: Subscriber): void {
    this.readers.delete(subscriber);
  }

  /** A person by their name, as a reference. */
  userOfName(username: string): UserRef {
    return this.userOf(username);
  }

  private userOf(username: string): UserRef {
    const person = this.person(username);
    if (person === undefined) return { id: fixtureId(1, 0), kind: 'human', username, display_name: username, avatar: { text: username.slice(0, 1).toUpperCase(), color: 0 } };
    return userRef(person.me);
  }

  // ---------------------------------------------------------------- who holds what

  /** The role a person holds along the path of a node. */
  roleAt(person: Person, nodeId: string): Role | null {
    const root = this.rootOf(nodeId);
    if (root === null) return null;
    if (root === person.me.personal_org_id) return 'admin';
    const domain = this.roles.get(root)?.get(person.me.username)?.role ?? null;
    // A role somebody gave in a place along the way (a person standing under People there) reaches everything inside it.
    const given = this.world.access.givenRoleAlong(person, nodeId);
    if (given === null) return domain;
    return domain === null || ROLES.indexOf(given) > ROLES.indexOf(domain) ? given : domain;
  }

  isMember(person: Person, rootId: string): boolean {
    return rootId === person.me.personal_org_id || this.roles.get(rootId)?.has(person.me.username) === true;
  }

  /** Explicit ordinary work here or below, plus follows; inherited View alone does not count. */
  participates(person: Person, nodeId: string): boolean {
    const reader = { ...person, me: { ...person.me, is_system_admin: false } };
    const first = this.rowOf(nodeId);
    if (first === undefined) return false;
    return [first, ...this.descendants(nodeId)].some((row) => {
      if (row.deleted || this.pathTo(row.id).some((step) => step.deleted)) return false;
      const root = this.rootOf(row.id);
      const domain = root === null ? undefined : this.roles.get(root)?.get(person.me.username)?.domain;
      const owns = this.metaOf(row.id).owner === person.me.username && root !== null && this.isMember(reader, root);
      const subscribed = this.subscriptions.get(`${person.me.username}:${row.id}`);
      const chat = this.world.chats.states.get(`${person.me.username}:${row.chat.id}`);
      return row.id === domain || row.id === person.me.personal_org_id || owns
        || this.world.access.namedAt(reader, row.id)
        || [...(this.granted.get(person.me.username) ?? [])].some((entry) => entry.endsWith(`@${row.id}`))
        || (subscribed !== undefined && subscribed !== 'off') || (chat !== undefined && chat.effective_level !== 'off');
    });
  }

  access(person: Person, nodeId: string, includeWithheld = false): Access {
    const row = this.row(nodeId);
    const kind: NodeKind = row?.kind ?? 'org';
    const role = this.roleAt(person, nodeId);
    const system = person.me.is_system_admin;
    const gone = this.denied.get(person.me.username);
    // What the node's own page does besides what the role gives: those named on it hold what it names, and a page cut loose closes the node to the rest.
    const given = [...(this.granted.get(person.me.username) ?? [])].flatMap((entry) => (entry.endsWith(`@${nodeId}`) ? [entry.slice(0, -nodeId.length - 1)] : []));
    const project = (native: boolean): string[] => {
      const reader = native ? person : { ...person, me: { ...person.me, is_system_admin: false } };
      const effect = this.world.access.pageEffect(reader, nodeId);
      const readerRole = native ? role : this.roleAt(reader, nodeId);
      return [...new Set([...(effect.closed ? [] : [...permissionsOf(kind, readerRole, native), ...documentRows(readerRole, native)]), ...effect.adds, ...given])]
        .filter((permission) => !gone?.has(permission) && !gone?.has(`${permission}@${nodeId}`))
        .filter((permission) => includeWithheld || row?.archived !== true || permission.endsWith('.view') || permission.endsWith('.export')
          || [`${kind}.manage`, `${kind}.archive`, 'access.manage', 'access.own', 'usage.summary'].includes(permission))
        .sort();
    };
    const permissions = project(system);
    const ordinary = new Set(system ? project(false) : permissions);
    const own = this.subscriptions.get(`${person.me.username}:${nodeId}`) ?? null;
    const ancestor = this.pathTo(nodeId).slice(0, -1).reverse().find((place) => this.subscriptions.has(`${person.me.username}:${place.id}`));
    const inherited = ancestor === undefined ? 'off' : this.subscriptions.get(`${person.me.username}:${ancestor.id}`) ?? 'off';
    return {
      permissions,
      admin_only: permissions.filter((permission) => !ordinary.has(permission)),
      participating: this.participates(person, nodeId),
      role: system ? 'admin' : role,
      role_from: system ? 'system' : null,
      administers: role === 'admin' || system,
      owns: this.metaOf(nodeId).owner === person.me.username,
      subscription: own,
      effective_subscription: own ?? inherited,
      inherited_subscription: inherited,
    };
  }

  /** Refuses as the real API does: what is required, of what, and everything that is missing with whom to ask. */
  need(person: Person, nodeId: string, permission: string, allowArchived = false): void {
    if (this.access(person, nodeId, true).permissions.includes(permission)) {
      if (!allowArchived && this.row(nodeId)?.archived === true && !permission.endsWith('.view') && !permission.endsWith('.export')
        && !['access.manage', 'access.own', 'usage.summary'].includes(permission)) throw new Refusal(409, 'node.archived', 'This place is archived and read-only.');
      return;
    }
    const row = this.row(nodeId);
    const owner = this.userOf(this.metaOf(nodeId).owner);
    const object = { kind: row?.kind ?? 'org', id: nodeId };
    throw new Refusal(403, 'authz.denied', `${permission} is required on this ${object.kind}`, {
      required: permission,
      object,
      requestable: true,
      override: false,
      missing: [{ object: { ...object, name: row?.name ?? null, owner }, permissions: [permission], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
    });
  }

  // ---------------------------------------------------------------- shapes

  /** The address of an org: the short names of the orgs from the root down to it, joined by dots. The one place that works it out, for the tree, the pages, the members and the finder. */
  addressOf(id: string): string {
    return this.pathTo(id)
      .filter((row) => row.kind === 'org')
      .map((row) => this.metaOf(row.id).slug ?? row.name.toLowerCase())
      .join('.');
  }

  /** What every node says of itself, for the kinds this file does not shape. */
  base(person: Person, row: TreeNode) {
    const meta = this.metaOf(row.id);
    const readme = meta.readmeDocumentId === null || meta.readmeDocumentId === undefined ? undefined : this.world.documents.docs.get(meta.readmeDocumentId);
    const path = this.pathTo(row.id).map((entry) => (entry.id === row.id ? this.ref(entry) : this.refFor(person, entry)));
    return {
      id: row.id,
      parent_id: row.parent_id,
      org_id: path[0]?.id ?? row.id,
      path,
      name: row.name,
      ...(row.deleted ? { inspection: { deleted_at: meta.deletedAt ?? MADE_AT, deleted_by: this.userOf(meta.deletedBy ?? meta.owner), purge_at: new Date(Date.parse(meta.deletedAt ?? MADE_AT) + DELETE_KEEP_MS).toISOString(), readonly: true as const } } : {}),
      description: meta.description,
      color: row.color,
      ...(typeof row.custom_color === 'string' ? { custom_color: row.custom_color } : {}),
      readme: readme !== undefined && !readme.deleted && readme.nodeId === row.id && this.world.documents.full(person, readme).access.permissions.includes('doc.view') ? this.world.documents.ref(person, readme) : null,
      owner: this.userOf(meta.owner),
      owners: 1,
      title_source: 'user' as const,
      description_source: 'user' as const,
      tags: meta.tagIds.flatMap((tag) => this.tags.get(tag) ?? []),
      tags_source: 'user' as const,
      auto_updated_at: null,
      page: 'follows' as const,
      chat_id: row.chat.id,
      archived: row.archived,
      archived_at: meta.archivedItself ? meta.updatedAt : null,
      archived_by: meta.archivedItself ? this.userOf(meta.owner) : null,
      revision: meta.revision,
      created_at: meta.createdAt,
      updated_at: meta.updatedAt,
      access: this.access(person, row.id),
    };
  }

  /** The live direct threads used by topic summaries and announcement delivery. */
  threadsOf(topicId: string): TreeNode[] {
    return this.children(topicId).filter((row) => row.kind === 'thread');
  }

  org(person: Person, row: TreeNode): Org {
    const base = this.base(person, row);
    const root = base.path[0]?.id === row.id;
    const personal = root && row.id === person.me.personal_org_id;
    const projects = this.children(row.id).filter((child) => child.kind === 'project');
    const orgs = this.children(row.id).filter((child) => child.kind === 'org');
    const rootId = base.org_id;
    const members = [...(this.roles.get(rootId)?.values() ?? [])].filter((entry) => root || this.isInside(entry.domain, row.id)).length;
    return {
      ...base,
      kind: 'org',
      slug: this.metaOf(row.id).slug ?? row.name.toLowerCase(),
      address: this.addressOf(row.id),
      depth: base.path.length - 1,
      org_kind: personal || this.rootIsPersonal(rootId) ? 'personal' : 'team',
      closed: this.rootIsPersonal(rootId),
      flat: this.rootIsPersonal(rootId),
      member_count: this.rootIsPersonal(rootId) ? 1 : members,
      people_count: [...this.world.people.values()].filter((other) => {
        if (other.suspended || other.me.kind !== 'human') return false;
        if (this.rootIsPersonal(rootId)) return other.me.id === base.owner.id;
        const domain = this.roles.get(rootId)?.get(other.me.username)?.domain;
        return (domain !== undefined && (this.isInside(domain, row.id) || this.isInside(row.id, domain))) || this.world.access.givenRoleAlong(other, row.id) !== null;
      }).length,
      org_count: orgs.length,
      project_count: projects.length,
      // A minute for each change of its root's tree: the later a tree last changed, the later this is.
      changed_at: new Date(Date.parse(MADE_AT) + (this.world.trees.get(rootId)?.seq ?? 0) * 60_000).toISOString(),
    };
  }

  private rootIsPersonal(rootId: string): boolean {
    return this.world.orgs.some((org) => org.id === rootId && org.org_kind === 'personal');
  }

  private isInside(nodeId: string, placeId: string): boolean {
    return this.pathTo(nodeId).some((row) => row.id === placeId);
  }

  project(person: Person, row: TreeNode): Project {
    const inside = this.children(row.id);
    return {
      ...this.base(person, row),
      kind: 'project',
      topic_count: inside.filter((child) => child.kind === 'topic').length,
      project_count: inside.filter((child) => child.kind === 'project').length,
    };
  }

  topic(person: Person, row: TreeNode): Topic {
    const meta = this.metaOf(row.id);
    const threads = this.threadsOf(row.id);
    const states = (state: string) => threads.filter((thread) => thread.thread?.state === state).length;
    const last = threads.map((thread) => thread.thread?.last_activity_at ?? null).filter((at): at is string => at !== null).sort().at(-1) ?? null;
    return {
      ...this.base(person, row),
      kind: 'topic',
      thread_display_state: (['waiting_approval','blocked','failed','running','queued','held','finished'] as const).find((state) => threads.some((thread) => this.access(person, thread.id).permissions.includes('thread.view') && thread.thread?.display.state === state)) ?? null,
      status: row.topic?.status ?? 'open',
      seq: meta.seq,
      thread_count: threads.length,
      attention: { blocked: states('blocked'), running: states('running') + states('starting'), waiting_approval: states('waiting_approval') },
      work_attention: {
        uncommitted: threads.filter((thread) => this.access(person, thread.id).permissions.includes('thread.view') && thread.thread?.attention.uncommitted === true).length,
        unpushed: threads.filter((thread) => this.access(person, thread.id).permissions.includes('thread.view') && thread.thread?.attention.unpushed === true).length,
        failed: threads.filter((thread) => this.access(person, thread.id).permissions.includes('thread.view') && thread.thread?.attention.failed === true).length,
      },
      done_at: meta.doneAt ?? null,
      last_activity_at: last,
      default_environment_id: typeof this.world.settings.valueAt(row.id, 'env.default_id').value === 'string' ? String(this.world.settings.valueAt(row.id, 'env.default_id').value) : null,
      // The environments the threads of a topic run in are what the resources say they are bound to.
      environments: this.world.resources.environmentsOfTopic(row.id),
    };
  }

  node(person: Person, id: string, inspectDeleted = false): Node {
    const row = this.row(id);
    if (row === undefined || (row.deleted && !inspectDeleted)) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    this.need(person, id, `${row.kind}.view`);
    switch (row.kind) {
      case 'org':
        return this.org(person, row);
      case 'project':
        return this.project(person, row);
      case 'topic':
        return this.topic(person, row);
      default:
        return this.world.threads.node(person, row);
    }
  }

  // ---------------------------------------------------------------- pages

  private page<Item>(items: readonly Item[], query: URLSearchParams): { items: Item[]; next_cursor: string | null } {
    const limit = Math.min(PAGE_MAX, Math.max(1, Number(query.get('limit') ?? PAGE_DEFAULT)));
    const from = Number(query.get('cursor') ?? 0);
    const next = from + limit;
    return { items: items.slice(from, next), next_cursor: next < items.length ? String(next) : null };
  }

  // ---------------------------------------------------------------- recording

  private log(person: Person, action: string, row: TreeNode, title: string, kind: Line['kind'] = 'history'): void {
    this.clock += 1;
    this.lines.unshift({
      at: new Date(Date.parse('2026-10-11T10:00:00.000Z') + this.clock * 1_000).toISOString(),
      actor: person.me.username,
      action,
      title,
      node: row.id,
      parent: row.parent_id,
      kind,
      thread: null,
    });
  }

  private touch(row: TreeNode, more: Partial<Meta> = {}): Meta {
    const meta = this.metaOf(row.id);
    Object.assign(meta, more, { revision: meta.revision + 1, updatedAt: new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString() });
    return meta;
  }

  /** Changes rows of a tree as the real system does, so that the left bar and everybody who follows it see the change. */
  private change(rootId: string, changes: (Partial<TreeNode> & { id: string })[]): void {
    this.world.changeTree(rootId, changes);
  }

  private fresh(kind: NodeKind, name: string, parentId: string, extra: Partial<TreeNode> = {}): TreeNode {
    const id = randomUUID();
    return {
      id,
      kind,
      name,
      address: null,
      parent_id: parentId,
      color: null,
      docs: 0,
      archived: false,
      deleted: false,
      restricted: false,
      participating: true,
      chat: { id: randomUUID(), seq: 0 },
      thread: null,
      topic: kind === 'topic' ? { status: 'open', tag_ids: [] } : null,
      tree_seq: 0,
      ...extra,
    };
  }

  private invalid(path: string, message: string): never {
    throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
  }

  // ---------------------------------------------------------------- the requests

  /** Answers a request about places, or `null` when it is not one. */
  /**
   * Who a person may name, found by what they typed (two characters at least), as the real server answers: in an org, its members by the start of
   * their username or a part of their name; with no org, the people of the orgs the caller shares by the same, and anybody at all by the whole of
   * a username or an address.
   */
  private searchPeople(person: Person, query: URLSearchParams): UserRef[] {
    const q = (query.get('q') ?? '').trim().toLowerCase();
    if (Array.from(q).length < 2) throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path: 'q', message: 'must be at least 2 characters' }] });
    const orgId = query.get('org_id');
    const inOrg = orgId === null ? null : new Set(this.roles.get(this.rootOf(orgId) ?? orgId)?.keys() ?? []);
    const shared = new Set<string>();
    for (const [root, members] of this.roles) if (this.isMember(person, root)) for (const username of members.keys()) shared.add(username);
    return [...this.world.people.values()]
      .filter((other) => {
        if (other.suspended) return false;
        const exact = other.me.username.toLowerCase() === q || (other.me.email ?? '').toLowerCase() === q;
        const near = other.me.username.toLowerCase().startsWith(q) || other.me.display_name.toLowerCase().includes(q);
        if (inOrg !== null) return near && inOrg.has(other.me.username);
        return exact || (near && shared.has(other.me.username));
      })
      .slice(0, 20)
      .map((other) => userRef(other.me));
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    const text = (member: string): string | null => (typeof body[member] === 'string' ? body[member] : null);
    let found: RegExpExecArray | null;

    if (route === 'GET /users/search') return { status: 200, body: this.searchPeople(person, query) };
    // Native UserGet is an authenticated exact-ID public reference, independent of name search scope.
    if ((found = /^GET \/users\/([^/]+)$/.exec(route))) {
      const selected = [...this.world.people.values()].find((candidate) => candidate.me.id === found?.[1]);
      if (selected === undefined) throw new Refusal(404, 'not_found', 'no such user');
      return { status: 200, body: { ...userRef(selected.me), address: `@${selected.me.username}` } };
    }
    if (route === 'GET /me/favorites') return { status: 200, body: this.favoriteList(person) };
    if (route === 'GET /shared') return { status: 200, body: { items: [], partial: false, next_cursor: null } };
    if (route === 'POST /orgs') {
      // Who may make a team org is the system's `orgs.created_by`, held to here as the server does; an
      // invitation that lets the person is used up only by an org that was made.
      if (!this.world.admin.mayMakeOrg(person)) {
        throw new Refusal(403, 'org.creation_disabled', 'making a team org is restricted here', { org_creation: this.world.meta.org_creation });
      }
      const made = this.makeRoot(person, text('name'), text('slug'));
      this.world.admin.takeOrgMaking(person);
      return { status: 200, body: made };
    }

    if ((found = /^PUT \/me\/favorites\/([^/]+)$/.exec(route))) {
      const list = this.favorites.get(person.me.username) ?? [];
      const id = found[1] ?? '';
      this.node(person, id);
      const key = `${this.must(id).kind}:${id}`;
      const without = list.filter((entry) => entry !== key);
      const after = body.after_node_id;
      if (after === undefined) without.push(key);
      else if (after === null) without.unshift(key);
      else {
        const afterId = asText(after);
        const at = without.indexOf(`${this.must(afterId).kind}:${afterId}`);
        if (at < 0) this.invalid('after_node_id', 'Choose another favorite.');
        without.splice(at + 1, 0, key);
      }
      this.favorites.set(person.me.username, without);
      this.world.users.tell(person.me.username, { what: 'favorites' });
      return { status: 204 };
    }
    if ((found = /^DELETE \/me\/favorites\/([^/]+)$/.exec(route))) {
      const id = found[1] ?? '';
      const key = `${this.must(id).kind}:${id}`;
      this.favorites.set(person.me.username, (this.favorites.get(person.me.username) ?? []).filter((entry) => entry !== key));
      this.world.users.tell(person.me.username, { what: 'favorites' });
      return { status: 204 };
    }
    if (route === 'GET /me/subscriptions') return { status: 200, body: this.subscriptionList(person, query.get('org_id')) };
    if (route === 'GET /me/notification-exceptions') return {status:200,body:this.notificationExceptions(person,query)};
    if ((found=/^PUT \/me\/notification-exceptions\/([^/]+)$/.exec(route))) {
      const id=found[1] ?? '';
      if(body.level!==null && body.level!=='all' && body.level!=='important' && body.level!=='off')this.invalid('level','Choose a notification level or its default.');
      // Validate the requested page before the in-memory producer writes, matching native rollback.
      this.notificationExceptions(person,query);
      const access=this.changeSubscription(person,id,body.level);
      return {status:200,body:{access,exceptions:this.notificationExceptions(person,query),...(query.has('cursor')?{page_cursor:query.get('cursor')}:{})}};
    }
    // A document made, moved, deleted or brought back changes how many stand beside a node: the documents answer the
    // request, and the rows of the nodes it changed are sent again once they have.
    if (method !== 'GET' && (/^\/nodes\/[^/]+\/documents$/.test(path) || /^\/documents\/[^/]+(\/(restore|move))?$/.test(path))) {
      const before = this.documentCounts();
      setImmediate(() => this.documentsChanged(before));
    }

    if ((found = /^GET \/nodes\/([^/]+)$/.exec(route))) {
      const id = found[1] ?? '';
      if (this.row(id)?.deleted === true && query.get('inspect_deleted') === 'true') {
        const row = this.must(id);
        if (row.parent_id !== null) this.need(person, row.parent_id, `${this.must(row.parent_id).kind}.manage`);
        else this.need(person, id, `${row.kind}.manage`);
        return { status: 200, body: this.node(person, id, true) };
      }
      return { status: 200, body: this.node(person, id) };
    }
    if ((found = /^PATCH \/nodes\/([^/]+)$/.exec(route))) return { status: 200, body: this.rename(person, found[1] ?? '', body) };
    if ((found = /^DELETE \/nodes\/([^/]+)$/.exec(route))) return this.remove(person, found[1] ?? '', body);
    if ((found = /^POST \/nodes\/([^/]+)\/delete\/preview$/.exec(route))) return this.deletePlan(person, found[1] ?? '');
    if ((found = /^POST \/nodes\/([^/]+)\/(archive|unarchive)\/preview$/.exec(route))) return { status: 200, body: this.archivePlan(person, found[1] ?? '', found[2] === 'archive') };
    if ((found = /^POST \/nodes\/([^/]+)\/(archive|unarchive)$/.exec(route))) return { status: 200, body: this.applyArchive(person, found[1] ?? '', found[2] === 'archive', body) };
    if ((found = /^POST \/nodes\/([^/]+)\/restore$/.exec(route))) return { status: 200, body: this.restore(person, found[1] ?? '', body) };
    if ((found = /^GET \/nodes\/([^/]+)\/deleted$/.exec(route))) return { status: 200, body: this.deletedUnder(person, found[1] ?? '', query) };
    if ((found = /^GET \/nodes\/([^/]+)\/children$/.exec(route))) return { status: 200, body: this.childList(person, found[1] ?? '', query) };
    if ((found = /^GET \/nodes\/([^/]+)\/activity$/.exec(route))) return { status: 200, body: this.activity(person, found[1] ?? '', query) };
    if ((found = /^GET \/nodes\/([^/]+)\/tags$/.exec(route))) return { status: 200, body: this.tagsFor(person, found[1] ?? '') };
    if ((found = /^POST \/nodes\/([^/]+)\/tags$/.exec(route))) return { status: 200, body: this.makeTag(person, found[1] ?? '', body) };
    if ((found = /^PUT \/nodes\/([^/]+)\/tags$/.exec(route))) return { status: 200, body: this.setTags(person, found[1] ?? '', body) };
    if ((found = /^(PATCH|DELETE) \/tags\/([^/]+)$/.exec(route))) return this.changeTag(person, found[2] ?? '', found[1] === 'DELETE', body);
    if ((found = /^GET \/nodes\/([^/]+)\/roles$/.exec(route))) return { status: 200, body: this.roleList(person, found[1] ?? '') };
    if ((found = /^(PUT|DELETE) \/nodes\/([^/]+)\/subscription$/.exec(route))) {
      const id = found[2] ?? '';
      return { status: 200, body: this.changeSubscription(person,id,found[1]==='PUT' && (body.level==='all'||body.level==='important'||body.level==='off')?body.level:null) };
    }

    if ((found = /^POST \/orgs\/([^/]+)\/orgs$/.exec(route))) return { status: 200, body: this.makeDepartment(person, found[1] ?? '', body) };
    if ((found = /^POST \/orgs\/([^/]+)\/projects$/.exec(route))) return { status: 200, body: this.makeProject(person, found[1] ?? '', body) };
    if ((found = /^POST \/projects\/([^/]+)\/projects$/.exec(route))) return { status: 200, body: this.makeFolder(person, found[1] ?? '', body) };
    if ((found = /^POST \/projects\/([^/]+)\/topics$/.exec(route))) {
      const parent = found[1] ?? '';
      const first = body.first_thread;
      if (first !== undefined && !isRecord(first)) this.invalid('first_thread', 'name a complete first thread');
      if (isRecord(first)) this.world.threads.startAdmission(person, parent, first);
      const topic = this.makeTopic(person, parent, body);
      if (!isRecord(first)) return { status: 200, body: topic };
      const made = this.world.threads.handle(person, 'POST', `/topics/${topic.id}/threads`, new URLSearchParams(), first);
      if (made === null) throw new Error('the admitted topic has no first thread');
      return { status: 200, body: { topic, thread: made.body } };
    }
    if ((found = /^GET \/projects\/([^/]+)\/topics$/.exec(route))) return { status: 200, body: this.topicList(person, found[1] ?? '', query) };
    if ((found = /^PATCH \/topics\/([^/]+)$/.exec(route))) return { status: 200, body: this.changeTopic(person, found[1] ?? '', body) };
    if ((found = /^POST \/(projects|orgs|topics)\/([^/]+)\/move\/preview$/.exec(route))) {
      this.ofKind(found[2] ?? '', found[1] ?? '');
      return { status: 200, body: this.movePlan(person, found[2] ?? '', asText(body.parent_id)) };
    }
    if ((found = /^POST \/(projects|orgs|topics)\/([^/]+)\/move$/.exec(route))) {
      this.ofKind(found[2] ?? '', found[1] ?? '');
      return { status: 200, body: this.move(person, found[2] ?? '', body) };
    }

    if (route === 'GET /invitations') return { status: 200, body: this.invitationList(person, query.get('org_id') ?? '') };
    if (route === 'POST /invitations') return { status: 200, body: this.invite(person, body) };
    if ((found = /^DELETE \/invitations\/([^/]+)$/.exec(route))) return this.revoke(person, found[1] ?? '');
    if ((found = /^PATCH \/roles\/([^/]+)$/.exec(route))) return { status: 200, body: this.changeRole(person, found[1] ?? '', body) };
    return null;
  }

  /**
   * The requests only the double has. `/grant`: a test gives somebody a permission on one node, as a page that names them there
   * would. `/follow`: somebody sets how much of a node they want to be told (`level`, or none to take the choice back), as if on
   * another of their devices.
   */
  control(path: string, body: Record<string, unknown>): Answer | null {
    const username = asText(body.username);
    const node = asText(body.node);
    if (path === '/grant') {
      const permissions = Array.isArray(body.permissions) ? body.permissions.filter((one): one is string => typeof one === 'string') : [];
      const held = this.granted.get(username) ?? new Set<string>();
      for (const permission of permissions) held.add(`${permission}@${node}`);
      this.granted.set(username, held);
      return { status: 204 };
    }
    if (path === '/follow') {
      const key = `${username}:${node}`;
      if (body.level === 'all' || body.level === 'important' || body.level === 'off') this.subscriptions.set(key, body.level);
      else this.subscriptions.delete(key);
      this.world.users.tell(username, { what: 'subscription', node_id: node, level: this.subscriptions.get(key) ?? null });
      return { status: 204 };
    }
    return null;
  }

  /** Sends again the row of every node beside which the number of documents changed since `before` was counted. */
  private documentsChanged(before: ReadonlyMap<string, number>): void {
    const after = this.documentCounts();
    for (const id of new Set([...before.keys(), ...after.keys()])) {
      if ((before.get(id) ?? 0) === (after.get(id) ?? 0)) continue;
      const root = this.rootOf(id);
      if (root !== null) this.change(root, [{ id }]);
    }
  }

  // ---------------------------------------------------------------- reads

  /** The levels a person set themself on nodes, all of them or those of one tree. */
  private subscriptionList(person: Person, orgId: string | null): { node_id: string; level: 'all' | 'important' | 'off'; source: 'manual' }[] {
    const mine = `${person.me.username}:`;
    return [...this.subscriptions.entries()]
      .filter(([key]) => key.startsWith(mine))
      .map(([key, level]) => ({ node_id: key.slice(mine.length), level, source: 'manual' as const }))
      .filter((entry) => orgId === null || this.rootOf(entry.node_id) === orgId);
  }

  private changeSubscription(person:Person,id:string,level:'all'|'important'|'off'|null):Access {
    this.node(person,id);const key=`${person.me.username}:${id}`;
    if(level===null)this.subscriptions.delete(key);else this.subscriptions.set(key,level);
    this.world.users.tell(person.me.username,{what:'subscription',node_id:id,level});
    return this.access(person,id);
  }

  private notificationExceptions(person:Person,query:URLSearchParams):{items:NotificationException[];next_cursor:string|null} {
    const prefix='notification-exceptions.v1:';
    const decoded=query.has('cursor')?Buffer.from(query.get('cursor') ?? '','base64url').toString():null;
    if(decoded!==null && !decoded.startsWith(prefix))this.invalid('cursor','This is not a notification exception page.');
    const after=decoded?.slice(prefix.length) ?? '';
    const rows=this.subscriptionList(person,null).flatMap((entry):NotificationException[]=>{
      const row=this.row(entry.node_id);
      if(row===undefined || row.deleted || !this.visibleTo(person,row))return [];
      const path=this.pathTo(row.id).slice(0,-1);
      const from=[...path].reverse().find((above)=>this.subscriptions.has(`${person.me.username}:${above.id}`));
      const inherited=from===undefined?'off':this.subscriptions.get(`${person.me.username}:${from.id}`) ?? 'off';
      return entry.level===inherited?[]:[{node:this.ref(row),path:path.map((part)=>this.refFor(person,part)),level:entry.level,
        default_level:inherited,default_from:from===undefined?null:this.refFor(person,from)}];
    }).filter((row)=>row.node.id>after).sort((one,other)=>one.node.id.localeCompare(other.node.id));
    const limit=Math.min(PAGE_MAX,Math.max(1,Number(query.get('limit') ?? PAGE_DEFAULT)));
    const items=rows.slice(0,limit);const last=items.at(-1);
    return {items,next_cursor:rows.length>limit && last!==undefined?Buffer.from(prefix+last.node.id).toString('base64url'):null};
  }

  favoriteList(person: Person): Favorite[] {
    return (this.favorites.get(person.me.username) ?? []).flatMap((key, index): Favorite[] => {
      const id = key.slice(key.indexOf(':') + 1);
      const row = this.row(id);
      if (row === undefined || row.deleted || key !== `${row.kind}:${row.id}`) return [];
      return [{ node: this.ref(row), path: this.pathTo(id).slice(0, -1).map((entry) => this.ref(entry)), position: String(index + 1), view: null }];
    });
  }

  childList(person: Person, id: string, query: URLSearchParams) {
    this.node(person, id);
    const archived = query.get('archived') ?? 'exclude';
    const kids = this.children(id)
      .filter((row) => row.kind === 'org' || row.kind === 'project')
      .filter((row) => (archived === 'include' ? true : archived === 'only' ? row.archived : !row.archived))
      .filter((row) => this.access(person, row.id).permissions.length > 0 || person.me.is_system_admin)
      .map((row) => (row.kind === 'org' ? this.org(person, row) : this.project(person, row)));
    return this.page(kids, query);
  }

  topicList(person: Person, projectId: string, query: URLSearchParams) {
    this.node(person, projectId);
    const status = query.get('status') ?? 'open';
    const archived = query.get('archived') ?? 'exclude';
    const wanted = query.getAll('tag_id');
    const attention = query.get('attention');
    const q = (query.get('q') ?? '').toLowerCase();
    const sort = query.get('sort') ?? 'activity';
    const scoped = query.get('with_folders') === 'true' ? this.descendants(projectId).filter((row) => {
      const parent = row.parent_id === null ? undefined : this.rowOf(row.parent_id);
      return parent?.kind === 'project' && this.access(person, parent.id).permissions.includes('project.view');
    }) : this.children(projectId);
    let topics = scoped
      .filter((row) => row.kind === 'topic')
      .filter((row) => !row.deleted && this.access(person, row.id).permissions.includes('topic.view'))
      .map((row) => {
        const topic = this.topic(person, row);
        return { ...topic, path: topic.path.map((part) => this.access(person, part.id).permissions.includes(`${part.kind}.view`) ? part : { ...part, name: null, redacted: true }) };
      });
    topics = topics
      .filter((topic) => status === 'all' || topic.status === status)
      .filter((topic) => (archived === 'include' ? true : archived === 'only' ? topic.archived : !topic.archived))
      .filter((topic) => wanted.every((tag) => topic.tags.some((held) => held.id === tag)))
      .filter((topic) => attention === null || (attention === 'needs_attention' ? topic.attention.blocked > 0 || topic.attention.waiting_approval > 0 : topic.attention[attention as 'running' | 'blocked' | 'waiting_approval'] > 0))
      .filter((topic) => q === '' || `${topic.name} ${topic.description}`.toLowerCase().includes(q));
    topics.sort((one, other) => {
      if (sort === 'name') return one.name.localeCompare(other.name);
      if (sort === 'created') return other.created_at.localeCompare(one.created_at) || other.seq - one.seq;
      return (other.last_activity_at ?? other.created_at).localeCompare(one.last_activity_at ?? one.created_at) || other.seq - one.seq;
    });
    return this.page(topics, query);
  }

  activity(person: Person, id: string, query: URLSearchParams) {
    this.node(person, id);
    const kinds = query.getAll('kind');
    // The history of a project takes in its folders: every project inside it is a place it answers for.
    const places = new Set([id]);
    if (this.row(id)?.kind === 'project') for (const inside of this.descendants(id)) if (inside.kind === 'project') places.add(inside.id);
    const lines = this.lines
      .filter((line) => places.has(line.node) || (line.parent !== null && places.has(line.parent)))
      .filter((line) => kinds.length === 0 || kinds.includes(line.kind))
      .map((line): ActivityEntry => {
        const row = this.row(line.node);
        const node = row === undefined ? { id: line.node, kind: 'topic' as const, name: null, redacted: true } : this.ref(row);
        const verbs: Readonly<Record<string, string>> = {
          'node.created': 'created', 'node.updated': 'changed', 'node.archived': 'archived', 'node.unarchived': 'unarchived',
          'node.deleted': 'deleted', 'node.restored': 'restored', 'node.moved': 'moved', 'topic.status_changed': 'status',
          'settings.updated': 'setting', 'settings.reset': 'setting', 'role.given': 'roles', 'role.changed': 'roles', 'role.removed': 'roles',
        };
        const workVerbs = { turn_completed: 'turnCompleted', turn_failed: 'turnFailed', milestone: 'milestone', announcement: 'said', approval_decided: 'decided' } as const;
        return {
          at: line.at,
          actor: this.userOf(line.actor),
          title: node.name ?? '',
          object: { id: node.id, kind: node.kind, name: node.name, redacted: node.redacted },
          wording: { version: 1, verb: verbs[line.action] ?? (line.kind === 'history' ? 'changed' : workVerbs[line.kind]), target: node },
          count: 1,
          node,
          kind: line.kind,
          action: line.action,
          thread: null,
          item_id: null,
        };
      });
    if (query.get('depth') === 'every') return this.page(lines, query);
    const groups = new Map<string, ActivityEntry>();
    for (const line of lines) {
      const actor = line.actor === null ? 'system' : 'agent' in line.actor ? line.actor.agent.id : line.actor.id;
      const hour = line.at.slice(0, 13);
      const key = `${actor}:${line.node.id}:${line.action ?? line.kind}:${hour}`;
      const held = groups.get(key);
      if (held === undefined) groups.set(key, line);
      else held.count += line.count;
    }
    return this.page([...groups.values()], query);
  }

  tagsFor(person: Person, id: string): Tag[] {
    this.node(person, id);
    const path = new Set(this.pathTo(id).map((row) => row.id));
    return [...this.tags.values()].filter((tag) => path.has(tag.node.id));
  }

  private makeTag(person: Person, nodeId: string, body: Record<string, unknown>): Tag {
    this.need(person, nodeId, `${this.row(nodeId)?.kind ?? 'org'}.manage`);
    const name = asText(body.name).trim();
    if (name === '') this.invalid('name', 'must not be empty');
    const own = [...this.tags.values()].filter((tag) => tag.node.id === nodeId);
    if (own.some((tag) => tag.name.toLocaleLowerCase() === name.toLocaleLowerCase())) throw new Refusal(409, 'conflict.exists', 'this tag name is already defined here');
    // Tags use the twelve zero-based colours of the existing identity palette, in its declared order.
    const paletteSize = 12;
    const used = new Set(own.map((tag) => tag.color));
    const firstUnused = Array.from({ length: paletteSize }, (_, at) => at).find((color) => !used.has(color));
    const color = typeof body.color === 'number' ? body.color : firstUnused ?? ((own.at(-1)?.color ?? -1) + 1) % paletteSize;
    const made: Tag = { id: randomUUID(), name, color, node: this.ref(this.must(nodeId)) };
    this.tags.set(made.id, made);
    return made;
  }

  /** A tag renamed or recoloured, or taken away from every topic that has it. */
  private changeTag(person: Person, id: string, remove: boolean, body: Record<string, unknown>): Answer {
    const tag = this.tags.get(id);
    if (tag === undefined) throw new Refusal(404, 'not_found', 'no such tag');
    this.need(person, tag.node.id, `${tag.node.kind}.manage`);
    if (remove) {
      this.tags.delete(tag.id);
      for (const meta of this.meta.values()) meta.tagIds = meta.tagIds.filter((one) => one !== tag.id);
      return { status: 204 };
    }
    if (typeof body.name === 'string' && body.name.trim() !== '') tag.name = body.name.trim();
    if (typeof body.color === 'number') tag.color = body.color;
    return { status: 200, body: tag };
  }

  deletedUnder(person: Person, id: string, query: URLSearchParams) {
    this.node(person, id);
    this.need(person, id, `${this.row(id)?.kind ?? 'org'}.manage`);
    const gone = this.children(id, true)
      .filter((row) => row.deleted && this.metaOf(row.id).deletedItself === true)
      .map((row) => ({
        node: this.ref(row),
        kind: row.kind,
        deleted_at: this.metaOf(row.id).deletedAt ?? MADE_AT,
        deleted_by: this.userOf(this.metaOf(row.id).deletedBy ?? 'ada'),
        purge_at: '2026-11-10T09:00:00.000Z',
        children: this.descendants(row.id).length,
      }));
    return this.page(gone, query);
  }

  roleList(person: Person, id: string) {
    this.node(person, id);
    const root = this.rootOf(id);
    const own: RoleHeld[] = [...(this.roles.get(root ?? '')?.entries() ?? [])]
      .filter(([, held]) => held.domain === id)
      .map(([username, held]) => ({
        id: held.roleId,
        node: this.ref(this.must(id)),
        subject: { kind: 'user', id: this.userOf(username).id, user: this.userOf(username) },
        role: held.role,
        domain: true,
        outside: false,
        expires_at: null,
        note: '',
        created_by: this.userOf('ada'),
        created_at: held.since,
        revision: held.revision,
      }));
    return { own, above: [] };
  }

  memberList(person: Person, orgId: string, query: URLSearchParams) {
    this.node(person, orgId);
    const root = this.rootOf(orgId) ?? '';
    const direct = query.get('direct') === 'true';
    const q = (query.get('q') ?? '').toLowerCase();
    const members = [...(this.roles.get(root)?.entries() ?? [])]
      .filter(([, held]) => (direct ? held.domain === orgId : orgId === root || this.isInside(held.domain, orgId)))
      .map(([username, held]): Member => {
        const domain = this.must(held.domain);
        const target = this.world.people.get(username);
        const manages = this.access(person, orgId).permissions.includes('members.manage');
        return {
          user: this.userOf(username),
          domain: this.ref(domain),
          address: `${this.addressOf(held.domain)}.${username}`,
          role: held.role,
          is_owner: this.metaOf(root).owner === username,
          since: held.since,
          also: 0,
          suspended: false,
          last_seen_at: null,
          second_step: !manages || target === undefined || target.me.kind === 'service' ? null : target.me.totp_enabled || target.me.email_code_second_step ? 'enabled' : 'missing',
          second_step_required: manages && target !== undefined && target.me.kind !== 'service' ? false : null,
        };
      })
      .filter((member) => q === '' || `${member.user.display_name} ${member.user.username}`.toLowerCase().includes(q))
      .sort((one, other) => one.user.display_name.localeCompare(other.user.display_name));
    return this.page(members, query);
  }

  invitationList(person: Person, orgId: string): Invitation[] {
    this.need(person, orgId, 'members.manage');
    return (this.invitations.get(orgId) ?? []).map((record) => record.invitation).reverse();
  }

  // ---------------------------------------------------------------- acts

  private requireName(body: Record<string, unknown>): string {
    const name = typeof body.name === 'string' ? body.name.trim() : '';
    if (name === '') this.invalid('name', 'must not be empty');
    return name;
  }

  private slugOf(body: Record<string, unknown>, fewest: number): string {
    const slug = typeof body.slug === 'string' ? body.slug : '';
    if (!/^[a-z0-9][a-z0-9_-]*$/.test(slug) || slug.length < fewest || slug.length > 32) {
      this.invalid('slug', `must be ${fewest} to 32 lower-case letters, digits, hyphens or underscores`);
    }
    return slug;
  }

  makeRoot(person: Person, name: string | null, slug: string | null): Org {
    const body = { name, slug };
    const cleanName = this.requireName(body);
    const cleanSlug = this.slugOf(body, 3);
    if (this.world.orgs.some((org) => org.slug === cleanSlug) || this.world.people.has(cleanSlug)) {
      // At the root another tree is not the person's to see: nobody is named, a free short name is offered.
      const taken = [...this.world.orgs.map((org) => org.slug), ...this.world.people.keys()];
      throw slugTaken(cleanSlug, null, taken);
    }
    const id = randomUUID();
    const template = this.world.orgs[0];
    if (template === undefined) throw new Refusal(500, 'internal', 'the fake has no org to copy');
    const root: Org = {
      ...template,
      id,
      org_id: id,
      slug: cleanSlug,
      address: cleanSlug,
      name: cleanName,
      description: '',
      path: [{ id, kind: 'org', name: cleanName, redacted: false }],
      org_kind: 'team',
      closed: false,
      flat: false,
      owner: userRef(person.me),
    };
    this.world.orgs.push(root);
    this.world.trees.set(id, { seq: 1, nodes: new Map([[id, { ...this.fresh('org', cleanName, '', { address: cleanSlug }), id, parent_id: null, tree_seq: 1 }]]) });
    this.metaOf(id).owner = person.me.username;
    this.metaOf(id).slug = cleanSlug;
    this.metaOf(id).createdAt = new Date(Date.parse(MADE_AT) + 60_000).toISOString();
    this.roles.set(id, new Map([[person.me.username, { role: 'admin', domain: id, since: this.metaOf(id).createdAt, roleId: randomUUID(), revision: 1 }]]));
    this.log(person, 'node.created', this.must(id), `${person.me.display_name} made the org ${cleanName}`);
    return this.org(person, this.must(id));
  }

  makeDepartment(person: Person, parentId: string, body: Record<string, unknown>): Org {
    this.node(person, parentId);
    this.need(person, parentId, 'org.manage');
    const name = this.requireName(body);
    const slug = this.slugOf(body, 2);
    if (this.rootIsPersonal(this.rootOf(parentId) ?? '')) throw new Refusal(409, 'org.flat', 'a personal org holds no org inside it');
    const siblings = this.children(parentId).filter((row) => row.kind === 'org');
    const holder = siblings.find((row) => (this.metaOf(row.id).slug ?? '') === slug);
    if (holder !== undefined) throw slugTaken(slug, holder, siblings.map((row) => this.metaOf(row.id).slug ?? ''));
    const row = this.fresh('org', name, parentId, { address: `${this.addressOf(parentId)}.${slug}` });
    const root = this.rootOf(parentId) ?? '';
    this.change(root, [row]);
    const meta = this.metaOf(row.id);
    meta.slug = slug;
    meta.owner = person.me.username;
    meta.description = typeof body.description === 'string' ? body.description : '';
    meta.createdAt = new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString();
    this.log(person, 'node.created', this.must(row.id), `${person.me.display_name} made the org ${name}`);
    return this.org(person, this.must(row.id));
  }

  makeProject(person: Person, parentId: string, body: Record<string, unknown>): Project {
    this.node(person, parentId);
    this.need(person, parentId, 'org.interact');
    const name = this.requireName(body);
    const row = this.fresh('project', name, parentId);
    this.change(this.rootOf(parentId) ?? '', [row]);
    const meta = this.metaOf(row.id);
    meta.owner = person.me.username;
    meta.description = typeof body.description === 'string' ? body.description : '';
    meta.createdAt = new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString();
    this.log(person, 'node.created', this.must(row.id), `${person.me.display_name} made the project ${name}`);
    return this.project(person, this.must(row.id));
  }

  /** A project made inside a project, beside its topics: what the screens call a folder. */
  makeFolder(person: Person, parentId: string, body: Record<string, unknown>): Project {
    const parent = this.row(parentId);
    if (parent === undefined || parent.deleted || parent.kind !== 'project') throw new Refusal(404, 'not_found', 'there is no project with that identifier');
    this.node(person, parentId);
    this.need(person, parentId, 'project.interact');
    const name = this.requireName(body);
    if (this.projectsOnPath(parentId) + 1 > PROJECT_DEPTH) throw this.tooDeep();
    const row = this.fresh('project', name, parentId);
    this.change(this.rootOf(parentId) ?? '', [row]);
    const meta = this.metaOf(row.id);
    meta.owner = person.me.username;
    meta.description = typeof body.description === 'string' ? body.description : '';
    meta.createdAt = new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString();
    this.log(person, 'node.created', this.must(row.id), `${person.me.display_name} made the folder ${name}`);
    return this.project(person, this.must(row.id));
  }

  makeTopic(person: Person, projectId: string, body: Record<string, unknown>): Topic {
    this.node(person, projectId);
    this.need(person, projectId, 'project.interact');
    const given = typeof body.name === 'string' ? body.name.trim() : '';
    const name = given === '' ? 'New topic' : given;
    const row = this.fresh('topic', name, projectId);
    this.change(this.rootOf(projectId) ?? '', [row]);
    const meta = this.metaOf(row.id);
    meta.owner = person.me.username;
    meta.description = typeof body.description === 'string' ? body.description : '';
    meta.seq = this.children(projectId, true).filter((entry) => entry.kind === 'topic').length;
    meta.createdAt = new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString();
    this.log(person, 'node.created', this.must(row.id), `${person.me.display_name} made the topic ${name}`);
    return this.topic(person, this.must(row.id));
  }

  /** A thread in a topic, as the tree holds it: the rest is the thread world's to say. */
  makeThreadRow(person: Person, topicId: string, name: string): TreeNode {
    this.node(person, topicId);
    const row = this.fresh('thread', name, topicId, { thread: { state: 'idle', display: display('idle'), attention: { uncommitted: null, unpushed: null, failed: false, as_of: null, stale: false, reason: null }, blocked_reason: null, pending_approvals: 0, activity_seq: 1, last_activity_at: new Date().toISOString() } });
    this.change(this.rootOf(topicId) ?? '', [row]);
    const meta = this.metaOf(row.id);
    meta.owner = person.me.username;
    meta.createdAt = new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString();
    this.log(person, 'node.created', this.must(row.id), `${person.me.display_name} made the thread ${name}`);
    return this.must(row.id);
  }

  rename(person: Person, id: string, body: Record<string, unknown>): Node {
    const before = this.node(person, id);
    const row = this.must(id);
    this.need(person, id, `${row.kind}.manage`);
    if (body.expected_revision !== before.revision) {
      throw new Refusal(409, 'conflict.stale_revision', 'the node was changed meanwhile', { current: before });
    }
    const name = typeof body.name === 'string' ? body.name.trim() : null;
    if (name === '') this.invalid('name', 'must not be empty');
    // The colour of its icon: one of the twelve, or null for the colour of its kind.
    const recolour = 'color' in body;
    if (recolour && body.color !== null && !(typeof body.color === 'number' && Number.isInteger(body.color) && body.color >= 1 && body.color <= 12)) {
      this.invalid('color', 'must be 1 to 12, or null');
    }
    // A colour of its own, #rrggbb kept in small letters, or null; one of the twelve replaces it and it replaces one of the twelve.
    const ownColour = 'custom_color' in body;
    if (ownColour && recolour) this.invalid('custom_color', 'give either a colour of the palette or one of its own, not both');
    if (ownColour && body.custom_color !== null && !(typeof body.custom_color === 'string' && /^#[0-9a-fA-F]{6}$/.test(body.custom_color))) {
      this.invalid('custom_color', 'must be #rrggbb, or null');
    }
    const own = typeof body.custom_color === 'string' ? body.custom_color.toLowerCase() : null;
    const old = row.name;
    if ('readme_document_id' in body) {
      const docId = body.readme_document_id;
      if (docId !== null && typeof docId !== 'string') this.invalid('readme_document_id', 'must be a document identifier or null');
      if (typeof docId === 'string') {
        const doc = this.world.documents.docs.get(docId);
        if (doc === undefined || doc.deleted || !this.world.documents.full(person, doc).access.permissions.includes('doc.view')) throw new Refusal(404, 'not_found', 'no such document');
        if (doc.nodeId !== id) throw new Refusal(409, 'node.wrong_parent', 'the README must be a document of this place');
      }
      this.metaOf(id).readmeDocumentId = docId;
    }
    if (name !== null || recolour || ownColour) {
      this.change(this.rootOf(id) ?? '', [{
        id,
        ...(name === null ? {} : { name }),
        ...(recolour ? { color: body.color as number | null, custom_color: null } : {}),
        ...(ownColour ? { color: null, custom_color: own } : {}),
      }]);
    }
    this.touch(row, typeof body.description === 'string' ? { description: body.description } : {});
    if (name !== null && name !== old) this.log(person, 'node.updated', this.must(id), `${person.me.display_name} renamed ${old} to ${name}`);
    return this.node(person, id);
  }

  changeTopic(person: Person, id: string, body: Record<string, unknown>): Topic {
    const row = this.row(id);
    if (row?.kind !== 'topic') throw new Refusal(404, 'not_found', 'no such topic');
    this.node(person, id);
    this.need(person, id, 'topic.interact');
    if (typeof body.expected_revision === 'number' && body.expected_revision !== this.metaOf(id).revision) throw new Refusal(409, 'conflict.stale_revision', 'the topic changed after it was reviewed');
    const root = this.rootOf(id) ?? '';
    if (body.status === 'open' || body.status === 'done') {
      this.change(root, [{ id, topic: { status: body.status, tag_ids: row.topic?.tag_ids ?? [] } }]);
      this.touch(row, { doneAt: body.status === 'done' ? new Date().toISOString() : undefined });
      this.log(person, 'topic.status_changed', this.must(id), `${person.me.display_name} marked ${body.status === 'done' ? 'done' : 'open'} ${row.name}`);
    }
    if (Array.isArray(body.tag_ids)) {
      this.setTags(person, id, body);
    }
    return this.topic(person, this.must(id));
  }

  private setTags(person: Person, id: string, body: Record<string, unknown>): Node {
    const row = this.must(id);
    this.need(person, id, `${row.kind}.interact`);
    if (row.archived) throw new Refusal(409, 'node.archived', 'This place is archived and read-only.');
    if (typeof body.expected_revision === 'number' && body.expected_revision !== this.metaOf(id).revision) throw new Refusal(409, 'conflict.stale_revision', 'The place changed after it was read.');
    if (!Array.isArray(body.tag_ids) || !body.tag_ids.every((tag): tag is string => typeof tag === 'string')) this.invalid('tag_ids', 'Give a list of tag identifiers.');
    const ids = [...new Set(body.tag_ids)];
    const limit = this.world.settings.valueAt(id, 'tags.max_per_place').value;
    if (typeof limit === 'number' && ids.length > limit) this.invalid('tag_ids', `A place carries at most ${limit} tags here.`);
    const above = new Set(this.pathTo(id).map((node) => node.id));
    const current = this.metaOf(id).tagIds;
    if (ids.some((tag) => !this.tags.has(tag) || (!above.has(this.tags.get(tag)!.node.id) && !current.includes(tag)))) this.invalid('tag_ids', 'Choose a tag defined here or above this place.');
    this.touch(row, { tagIds: ids });
    this.change(this.rootOf(id) ?? '', [{ id, ...(row.kind === 'topic' ? { topic: { status: row.topic?.status ?? 'open', tag_ids: ids } } : {}) }]);
    return this.node(person, id);
  }

  /** Identity, rather than counts alone, fences a reviewed subtree. */
  private reviewStamp(id: string): string {
    const rows = [this.must(id), ...this.descendants(id)].filter((row) => !row.deleted).sort((a, b) => a.id.localeCompare(b.id));
    const ids = new Set(rows.map((row) => row.id));
    const docs = [...this.world.documents.docs.values()].filter((doc) => ids.has(doc.nodeId) && !doc.deleted).map((doc) => [doc.id, doc.nodeId, doc.seq]).sort();
    const environments = this.world.resources.environments.filter((env) => ids.has(env.home) || [...this.world.resources.bindings].some(([thread, binding]) => ids.has(thread) && binding.environment === env.id)).map((env) => [env.id, env.revision]);
    const work = rows.filter((row) => row.kind === 'thread').map((row) => [row.id,this.world.threads.archiveIdentity(row.id)]);
    return createHash('sha256').update(JSON.stringify([rows.map((row) => [row, this.metaOf(row.id)]), this.pathTo(id).map((row) => [row.id, row.parent_id, this.metaOf(row.id).archivedItself]), docs, environments, work])).digest('hex');
  }

  archivePlan(person: Person, id: string, archiving: boolean): NodeArchivePlan {
    const row = this.must(id);
    this.node(person, id);
    const access = this.access(person, id).permissions;
    if (!access.includes(`${row.kind}.archive`) && !access.includes(`${row.kind}.manage`)) this.need(person, id, `${row.kind}.archive`);
    const rows = [row, ...this.descendants(id)].filter((one) => !one.deleted);
    const ids = new Set(rows.map((one) => one.id));
    const count = (kind: string) => rows.filter((one) => one.kind === kind).length;
    const plan: NodeArchivePlan = {
      preflight_id: randomUUID(), stamp: this.reviewStamp(id), expires_at: new Date(Date.now() + REVIEW_LIFE_MS).toISOString(), archiving,
      node: this.ref(row), revision: this.metaOf(id).revision,
      subtree: rows.map((one) => ({ node: this.ref(one), revision: this.metaOf(one.id).revision, own_archived: this.metaOf(one.id).archivedItself, read_only_after: this.pathTo(one.id).some((ancestor) => ancestor.id === id ? archiving : this.metaOf(ancestor.id).archivedItself) })),
      counts: { orgs: count('org'), projects: count('project'), topics: count('topic'), threads: count('thread'), documents: [...this.world.documents.docs.values()].filter((doc) => ids.has(doc.nodeId) && !doc.deleted).length },
      work: rows.filter((one) => one.kind === 'thread').map((one) => {
        const visible = this.access(person, one.id).permissions.includes('thread.view');
        const work = this.world.threads.archiveFacts(one.id);
        return { thread: visible ? this.ref(one) : { ...this.ref(one), name: null, redacted: true }, turns: visible ? work.turns : null, inputs: visible ? work.inputs : null, approvals: visible ? work.approvals : null, leases: visible ? work.leases : null, session: visible ? work.session : null, links: visible ? work.links : null };
      }),
      environments: this.world.resources.environments.filter((env) => ids.has(env.home) || [...this.world.resources.bindings].some(([thread, binding]) => ids.has(thread) && binding.environment === env.id)).map((env) => {
        const visible = this.world.access.permissionsOnEnvironment(person, env.id).includes('environment.view');
        return { id: env.id, name: visible ? env.name : null, redacted: !visible };
      }),
      copies_kept: 0, live_shares_kept: 0, blockers: [],
    };
    this.archiveReviews.set(plan.preflight_id, { user: person.me.id, plan });
    return plan;
  }

  private applyArchive(person: Person, id: string, on: boolean, body: Record<string, unknown>): NodeArchiveResult {
    const current = this.archivePlan(person, id, on);
    const held = this.archiveReviews.get(asText(body.preflight_id));
    if (held?.user !== person.me.id || held.plan.node.id !== id || held.plan.archiving !== on || Date.parse(held.plan.expires_at) <= Date.now()) throw new Refusal(409, 'preflight.expired', 'Review this archive action again.');
    this.archiveReviews.delete(held.plan.preflight_id);
    if (held.plan.stamp !== current.stamp || body.stamp !== held.plan.stamp || body.expected_revision !== current.revision) throw new Refusal(409, 'conflict.stale_revision', 'The subtree changed. Nothing was archived; review again.', { current });
    const changed = this.metaOf(id).archivedItself !== on;
    const node = this.archive(person, id, on);
    return { node, undo: changed ? this.archivePlan(person, id, !on) : null };
  }

  archive(person: Person, id: string, on: boolean): Node {
    const row = this.row(id);
    if (row === undefined || row.deleted) throw new Refusal(404, 'not_found', 'no such thing');
    this.node(person, id);
    const access = this.access(person, id).permissions;
    if (!access.includes(`${row.kind}.archive`) && !access.includes(`${row.kind}.manage`)) this.need(person, id, `${row.kind}.archive`);
    this.touch(row, { archivedItself: on });
    const changes = [row, ...this.descendants(id)].map((entry) => ({ id: entry.id, archived: this.pathTo(entry.id).some((ancestor) => this.metaOf(ancestor.id).archivedItself) }));
    this.change(this.rootOf(id) ?? '', changes);
    if (on) for (const entry of [row,...this.descendants(id)]) if (entry.kind === 'thread') this.world.threads.settleArchive(entry.id,person);
    this.log(person, on ? 'node.archived' : 'node.unarchived', this.must(id), `${person.me.display_name} ${on ? 'archived' : 'brought back'} ${row.name}`);
    return this.node(person, id);
  }

  remove(person: Person, id: string, body: Record<string, unknown> = {}): Answer {
    const row = this.row(id);
    if (row === undefined || row.deleted) throw new Refusal(404, 'not_found', 'no such thing');
    this.node(person, id);
    this.need(person, id, `${row.kind}.manage`, true);
    if (typeof body.preflight_id === 'string') {
      const plan = this.deleteReviews.get(body.preflight_id);
      if (plan?.user !== person.me.id || plan.node !== id || plan.expires <= Date.now()) throw new Refusal(409, 'preflight.expired', 'Review the deletion again.');
      this.deleteReviews.delete(body.preflight_id);
      if (plan.stamp !== this.reviewStamp(id) || body.expected_revision !== this.metaOf(id).revision) throw new Refusal(409, 'conflict.stale_revision', 'The subtree changed. Nothing was deleted; review again.', { current: this.deletePlan(person, id).body });
    }
    const rows = [row, ...this.descendants(id)].filter((entry) => !entry.deleted);
    // How each stood before the deletion archives it is kept under the deletion's group, which its Undo names.
    const group = randomUUID();
    this.deletions.set(id, { group, archived: new Map(rows.map((entry) => [entry.id, entry.archived])) });
    this.change(this.rootOf(id) ?? '', rows.map((entry) => ({ id: entry.id, deleted: true, archived: true })));
    this.touch(row, { deletedAt: new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString(), deletedBy: person.me.username, deletedItself: true });
    this.log(person, 'node.deleted', this.must(id), `${person.me.display_name} deleted the ${row.kind} ${row.name}`);
    return { status: 200, body: { deletion_id: group } };
  }

  /** The current deletion of each deleted place, with how each node of it stood before. */
  private deletions = new Map<string, { group: string; archived: Map<string, boolean> }>();

  /** What deleting a place would remove, with a caller-bound identity fence. */
  deletePlan(person: Person, id: string): Answer {
    const row = this.row(id);
    if (row === undefined || row.deleted) throw new Refusal(404, 'not_found', 'no such thing');
    this.node(person, id);
    this.need(person, id, `${row.kind}.manage`, true);
    const rows = [row, ...this.descendants(id)];
    const count = (kind: string) => rows.filter((entry) => entry.kind === kind).length;
    const parent = row.parent_id === null ? null : this.row(row.parent_id);
    const purgeAt = new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000 + 30 * 86_400_000).toISOString();
    const preflight = randomUUID();
    const expires = Date.now() + REVIEW_LIFE_MS;
    this.deleteReviews.set(preflight, { user: person.me.id, node: id, stamp: this.reviewStamp(id), expires });
    return {
      status: 200,
      body: {
        preflight_id: preflight,
        expires_at: new Date(expires).toISOString(),
        node: { id: row.id, kind: row.kind, name: row.name, redacted: false },
        revision: this.metaOf(id).revision,
        counts: { orgs: count('org'), projects: count('project'), topics: count('topic'), threads: count('thread'), documents: 0 },
        purge_at: purgeAt,
        restore_by:
          parent === undefined || parent === null
            ? { permission: 'access.own', place: { id: row.id, kind: row.kind, name: row.name, redacted: false } }
            : { permission: `${parent.kind}.manage`, place: { id: parent.id, kind: parent.kind, name: parent.name, redacted: false } },
      },
    };
  }

  restore(person: Person, id: string, body: Record<string, unknown> = {}): Node {
    const row = this.row(id);
    if (row?.deleted !== true) throw new Refusal(404, 'not_found', 'nothing was deleted there');
    const parent = row.parent_id;
    if (parent !== null) this.need(person, parent, `${this.row(parent)?.kind ?? 'org'}.manage`);
    const rows = [row, ...this.descendants(id)];
    // The Undo of the current deletion puts back how each stood; any other group is refused, and nothing comes back.
    const held = this.deletions.get(id);
    const undo = typeof body.undo_delete === 'string' ? body.undo_delete : null;
    if (undo !== null && held?.group !== undo) throw new Refusal(409, 'conflict.stale_revision', 'the deletion this undoes is no longer the current one', { current_deletion_id: held?.group ?? null });
    this.deletions.delete(id);
    const archivedBefore = (nodeId: string) => (undo === null || held === undefined ? true : (held.archived.get(nodeId) ?? true));
    this.change(this.rootOf(id) ?? '', rows.map((entry) => ({ id: entry.id, deleted: false, archived: archivedBefore(entry.id) })));
    this.touch(row, { deletedItself: false, archivedItself: archivedBefore(id) });
    this.log(person, 'node.restored', this.must(id), `${person.me.display_name} brought back ${row.name}`);
    return this.node(person, id);
  }

  /** What a place does to people when something is moved into it: only a few places are given an effect. */
  private moveEffects(): Map<string, MoveEffect> {
    const effects = new Map<string, MoveEffect>();
    effects.set(fixtureId(3, 19), {
      gaining: [{ username: 'linus', pages: 2 }],
      losing: [{ username: 'margaret', pages: 1 }],
      roles: { gaining: 1, losing: 1 },
      settings: [{ key: 'env.recycle_idle_minutes', from: 480, to: 60, lockedBy: fixtureId(3, 18) }],
    });
    return effects;
  }

  /** The request names a kind of node in its path; a node of another kind is not there. */
  private ofKind(id: string, plural: string): void {
    const want = plural === 'topics' ? 'topic' : plural === 'projects' ? 'project' : 'org';
    const row = this.row(id);
    if (row === undefined || row.deleted || row.kind !== want) throw new Refusal(404, 'not_found', `there is no ${want} with that identifier`);
  }

  /** How many projects lie one inside another on the way to a place, itself included when it is one. */
  projectsOnPath(id: string): number {
    return this.pathTo(id).filter((row) => row.kind === 'project').length;
  }

  /** How many projects lie one inside another from a project down to its deepest folder, itself included. */
  private projectHeight(id: string): number {
    let tallest = 0;
    for (const child of this.children(id)) if (child.kind === 'project') tallest = Math.max(tallest, this.projectHeight(child.id));
    return tallest + 1;
  }

  /** What the one who moves something must hold where it goes: the right to make that kind of thing there. */
  private needsWhereItGoes(moved: TreeNode, target: TreeNode): string {
    if (moved.kind === 'topic') return 'project.interact';
    if (moved.kind === 'project') return target.kind === 'org' ? 'org.interact' : 'project.interact';
    return 'org.manage';
  }

  private tooDeep(): Refusal {
    return new Refusal(409, 'project.too_deep', `a path holds at most ${PROJECT_DEPTH} projects one inside another`, { limit: PROJECT_DEPTH });
  }

  movePlan(person: Person, id: string, parentId: string): NodeMovePlan {
    const row = this.row(id);
    const target = this.row(parentId);
    if (row === undefined || row.deleted || target === undefined || target.deleted) throw new Refusal(404, 'not_found', 'no such thing');
    this.node(person, id);
    const from = this.row(row.parent_id ?? '');
    const blockers: PlanBlocker[] = [];
    // What a place of each kind may be put under, and what the one who moves it must hold where it goes.
    const lies = row.kind === 'topic' ? 'project' : row.kind === 'project' ? 'org or project' : 'org';
    const fits = row.kind === 'topic' ? target.kind === 'project' : row.kind === 'project' ? target.kind === 'org' || target.kind === 'project' : target.kind === 'org';
    if (this.rootOf(parentId) !== this.rootOf(id)) {
      blockers.push({ code: 'node.wrong_parent', message: 'it cannot leave its own tree of orgs' });
    } else if (this.isInside(parentId, id)) {
      blockers.push({ code: 'node.wrong_parent', message: `${row.name} cannot be moved into itself or into what is inside it` });
    } else if (!fits) {
      blockers.push({ code: 'node.wrong_parent', message: `a ${row.kind} lies in a ${lies}, and ${target.name} is a ${target.kind}` });
    } else if (row.kind === 'project' && this.projectsOnPath(parentId) + this.projectHeight(id) > PROJECT_DEPTH) {
      blockers.push({ code: 'project.too_deep', message: `a path holds at most ${PROJECT_DEPTH} projects one inside another` });
    } else {
      const wanted = this.needsWhereItGoes(row, target);
      if (!this.access(person, parentId).permissions.includes(wanted)) {
        const owner = this.userOf(this.metaOf(parentId).owner);
        blockers.push({
          code: 'authz.denied',
          message: 'you lack what the move needs where it goes',
          missing: [{ object: { kind: target.kind, id: parentId, name: target.name, owner }, permissions: [wanted], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
        });
      }
    }
    const effect = this.moveEffects().get(parentId);
    const people = (list: MoveEffect['gaining']): PreviewPerson[] => list.map((entry) => ({ user: this.userOf(entry.username), pages: entry.pages, permissions: row.kind === 'topic' ? ['topic.view', 'topic.interact'] : ['project.view'] }));
    const plan: NodeMovePlan = {
      preflight_id: randomUUID(), stamp: '', revision: this.metaOf(id).revision, expires_at: new Date(Date.now() + REVIEW_LIFE_MS).toISOString(),
      node: this.ref(row),
      from: this.ref(from ?? row),
      to: this.ref(target),
      access: { pages: this.descendants(id).length + 1, orgs: null, gaining: people(effect?.gaining ?? []), losing: people(effect?.losing ?? []), more: 0 },
      settings: (effect?.settings ?? []).map((setting) => {
        const lock = setting.lockedBy === null ? undefined : this.row(setting.lockedBy);
        return { key: setting.key, from: setting.from, to: setting.to, locked_by: lock === undefined ? null : this.ref(lock) };
      }),
      roles: effect?.roles ?? { gaining: 0, losing: 0 },
      addresses: { members: 0, resources: 0 },
      blockers,
    };
    const consequences = { ...plan, preflight_id: null, stamp: null, expires_at: null };
    plan.stamp = createHash('sha256').update(JSON.stringify([this.reviewStamp(id), this.pathTo(parentId).map((one) => [one.id, this.metaOf(one.id)]), consequences])).digest('hex');
    this.moveReviews.set(plan.preflight_id, { user: person.me.id, plan });
    return plan;
  }

  move(person: Person, id: string, body: Record<string, unknown>): Node {
    const plan = this.movePlan(person, id, asText(body.parent_id));
    const blocker = plan.blockers[0];
    if (blocker !== undefined) {
      if (blocker.code === 'project.too_deep') throw this.tooDeep();
      if (blocker.code === 'authz.denied') {
        const target = this.must(asText(body.parent_id));
        this.need(person, target.id, this.needsWhereItGoes(this.must(id), target));
      }
      throw new Refusal(409, 'node.wrong_parent', blocker.message);
    }
    const row = this.must(id);
    this.need(person, id, `${row.kind}.manage`);
    if (body.expected_revision !== this.metaOf(id).revision) {
      throw new Refusal(409, 'conflict.stale_revision', 'it was changed meanwhile', { current: this.node(person, id) });
    }
    if (typeof body.preflight_id === 'string') {
      const held = this.moveReviews.get(body.preflight_id);
      if (held?.user !== person.me.id || held.plan.node.id !== id || Date.parse(held.plan.expires_at) <= Date.now()) throw new Refusal(409, 'preflight.expired', 'Review this move again.');
      this.moveReviews.delete(body.preflight_id);
      if (held.plan.stamp !== plan.stamp || held.plan.to.id !== plan.to.id) throw new Refusal(409, 'conflict.stale_revision', 'The move consequences changed. Nothing moved; review again.', { current: plan });
    }
    const was = this.row(row.parent_id ?? '')?.name ?? '';
    this.change(this.rootOf(id) ?? '', [{ id, parent_id: asText(body.parent_id) }]);
    const addressed = [row, ...this.descendants(id)].filter((one) => one.kind === 'org');
    if (addressed.length > 0) this.change(this.rootOf(id) ?? '', addressed.map((one) => ({ id: one.id, address: this.addressOf(one.id) })));
    this.touch(row);
    this.log(person, 'node.moved', this.must(id), `${person.me.display_name} moved ${row.name} from ${was} to ${plan.to.name ?? ''}`);
    return this.node(person, id);
  }

  invite(person: Person, body: Record<string, unknown>) {
    const orgId = asText(body.org_id);
    this.node(person, orgId);
    this.need(person, orgId, 'members.manage');
    const root = this.rootOf(orgId) ?? '';
    if (this.rootIsPersonal(root)) throw new Refusal(409, 'org.closed', 'a personal org takes no members');
    const email = typeof body.email === 'string' && body.email !== '' ? body.email : null;
    if (email !== null && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) this.invalid('email', 'is not an address');
    const hours = typeof body.expires_in_hours === 'number' ? body.expires_in_hours : 168;
    if (hours > 720) this.invalid('expires_in_hours', 'at most 720 hours are allowed here');
    const token = `inv_${randomUUID()}`;
    const role = (ROLES as readonly string[]).includes(String(body.role)) ? (body.role as Role) : 'viewer';
    const row = this.must(orgId);
    const invitation: Invitation = {
      id: randomUUID(),
      kind: 'org',
      org: this.ref(row),
      node: null,
      chat: null,
      role,
      permissions: [],
      form: email === null ? 'link' : 'email',
      email,
      max_uses: email === null ? Number(body.max_uses ?? 1) : 1,
      uses: 0,
      note: typeof body.note === 'string' ? body.note : '',
      expires_at: new Date(Date.now() + hours * 3_600_000).toISOString(),
      revoked: false,
      created_by: userRef(person.me),
      created_at: new Date().toISOString(),
    };
    const list = this.invitations.get(orgId) ?? [];
    list.push({ invitation, token });
    this.invitations.set(orgId, list);
    return { invitation, token, url: `http://127.0.0.1:41872/invite/${token}`, sent: email !== null && this.mailWorks };
  }

  revoke(person: Person, invitationId: string): Answer {
    for (const [orgId, list] of this.invitations) {
      const found = list.find((record) => record.invitation.id === invitationId);
      if (found === undefined) continue;
      this.need(person, orgId, 'members.manage');
      found.invitation.revoked = true;
      return { status: 204 };
    }
    throw new Refusal(404, 'not_found', 'no such invitation');
  }

  changeRole(person: Person, roleId: string, body: Record<string, unknown>): RoleHeld {
    for (const members of this.roles.values()) {
      for (const [username, held] of members) {
        if (held.roleId !== roleId) continue;
        this.need(person, held.domain, 'access.manage');
        if (body.expected_revision !== held.revision) throw new Refusal(409, 'conflict.stale_revision', 'the role was changed meanwhile');
        if ((ROLES as readonly string[]).includes(String(body.role))) held.role = body.role as Role;
        held.revision += 1;
        const changed = this.roleList(person, held.domain).own.find((entry) => entry.subject.kind === 'user' && entry.subject.user?.username === username);
        if (changed === undefined) break;
        return changed;
      }
    }
    throw new Refusal(404, 'not_found', 'no such role');
  }

  /** Puts a person in a root org with a role: what accepting an invitation does. */
  join(username: string, rootId: string, role: Role, domain: string = rootId): void {
    const members = this.roles.get(rootId) ?? new Map<string, RoleOf>();
    members.set(username, { role, domain, since: new Date(Date.parse(MADE_AT) + (this.clock + 1) * 60_000).toISOString(), roleId: randomUUID(), revision: 1 });
    this.roles.set(rootId, members);
  }

  // ---------------------------------------------------------------- the world as a team at work

  /** Gives the team's tree what the real system holds beside its rows: owners, slugs, tags, people, a few lines of history. */
  seedTeam(): void {
    const acme = fixtureId(2, 100);
    const slugs: Record<string, string> = { [fixtureId(3, 1)]: 'research', [fixtureId(3, 13)]: 'ml' };
    for (const [nodeId, slug] of Object.entries(slugs)) this.metaOf(nodeId).slug = slug;
    this.metaOf(acme).slug = 'acme';
    const tag = (n: number, name: string, color: number, node: string): Tag => {
      const made: Tag = { id: fixtureId(5, n), name, color, node: { id: node, kind: 'org', name: 'Acme', redacted: false } };
      this.tags.set(made.id, made);
      return made;
    };
    const bug = tag(1, 'bug', 0, acme);
    tag(2, 'research', 6, acme);
    tag(3, 'urgent', 1, acme);
    const fix = this.row(fixtureId(3, 3));
    if (fix !== undefined) {
      this.metaOf(fix.id).tagIds = [bug.id];
      this.metaOf(fix.id).description = 'Sessions end too early';
      if (fix.topic !== null) fix.topic = { status: 'open', tag_ids: [bug.id] };
    }
    const seqOf: Record<string, number> = { [fixtureId(3, 3)]: 1, [fixtureId(3, 6)]: 2, [fixtureId(3, 8)]: 3, [fixtureId(3, 20)]: 1, [fixtureId(3, 21)]: 1 };
    this.metaOf(fixtureId(3, 17)).description = 'The storefront, from the cart to the receipt';
    this.metaOf(fixtureId(3, 18)).description = 'Everything between the cart and the receipt';
    this.metaOf(fixtureId(3, 19)).description = 'Cards, fees and refunds';
    for (const [nodeId, seq] of Object.entries(seqOf)) this.metaOf(nodeId).seq = seq;
    // Who is in Acme and with which role: every person of the fake system, so that every sign-in lands somewhere.
    const roles: Record<string, Role> = { ada: 'admin', grace: 'developer', linus: 'interactor', margaret: 'viewer', vera: 'viewer', dora: 'developer', bolted: 'viewer', unconfirmed: 'viewer', 王小明: 'developer' };
    for (const [username, role] of Object.entries(roles)) this.join(username, acme, role);
    const ada = this.world.people.get('ada');
    if (ada !== undefined) {
      this.log(ada, 'node.created', this.must(fixtureId(3, 1)), 'Ada Lovelace made the org Research');
      this.log(ada, 'node.created', this.must(fixtureId(3, 2)), 'Ada Lovelace made the project Web');
      this.log(ada, 'node.created', this.must(fixtureId(3, 3)), 'Ada Lovelace made the topic Fix login');
      this.log(ada, 'node.created', this.must(fixtureId(3, 17)), 'Ada Lovelace made the project Shop');
      this.log(ada, 'node.created', this.must(fixtureId(3, 18)), 'Ada Lovelace made the folder Checkout');
      this.log(ada, 'node.created', this.must(fixtureId(3, 19)), 'Ada Lovelace made the folder Payments');
    }
  }
}
