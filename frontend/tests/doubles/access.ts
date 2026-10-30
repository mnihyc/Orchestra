/** What the fake API says of access: the catalogue, the permission page of a node and of a provider, what a place gives to what is made inside it with locks, the change that edits either against a revision, previews, the people behind a chip, what one person holds, history, roles held in places, groups, finding people, and asking for access.
 *
 * It keeps to what the real API promises as far as a screen can tell: the same rows with what each
 * follows and from where, the same refusals (a stale revision answers with the page as it is now, a lock
 * above refuses a change, naming someone outside the org needs the right to), and the same
 * explicit fault scenes. Permission-page readers use the same object paths as the native API;
 * inspecting another person requires management of the page. What a page starts from is the system's shipped defaults.
 */
import { randomUUID } from 'node:crypto';

import type {
  AccessChoice,
  AgentLevel,
  Entry,
  Group,
  Holder,
  HolderInput,
  HolderCandidate,
  NodeOrSystem,
  NodeRef,
  PageChange,
  PageCovers,
  PageGives,
  PagePreview,
  PermissionDef,
  PermissionPage,
  PermissionRow,
  PreviewPerson,
  Role,
  RoleHeld,
  SharePreview,
  SharePreviewBody,
  Problem,
  RowProfile,
  UserRef,
} from '../../src/generated/api-types.ts';
import { CATALOGUE } from './catalogue.ts';
import { ACME, id as fixtureId, userRef } from './fixtures.ts';
import type { Resources, ThreadBinding } from './resources.ts';
import { Requests } from './requests.ts';
import { Shares } from './shares.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

const ROLES: readonly Role[] = ['viewer', 'interactor', 'developer', 'admin'];
const rank = (role: Role): number => ROLES.indexOf(role);

/** What a thread's screens ask of the environment it runs in: its own two levels and the rows of its four doors. */
const ENVIRONMENT_PERMISSIONS = ['browser.manage', 'browser.view', 'environment.manage', 'environment.use', 'environment.view', 'files.manage', 'files.view', 'terminal.manage', 'terminal.view', 'vnc.manage', 'vnc.view', 'ports.view', 'ports.manage'] as const;

/** The three everyday choices, from the least to the most. */
const CHOICES: readonly AccessChoice[] = ['full_view', 'full_interact', 'full_manage'];

/** The role a choice gives when it is given on a place with what is in it: the one whose work it names. */
export const ROLE_OF_CHOICE: Record<AccessChoice, Role> = { full_view: 'viewer', full_interact: 'interactor', full_manage: 'developer' };

/** How high a level stands in the ladder of its area; the advanced rows and the Owners row stand on none. */
const STEP: Record<PermissionDef['level'], number | null> = { view: 0, interact: 1, edit: 1, use: 1, manage: 2, own: null, advanced: null };

/** The permissions a person holds with these: every level below in the same area, and the advanced rows that belong with one held. */
function withLevels(held: ReadonlySet<string>): Set<string> {
  const all = new Set(held);
  const defs = CATALOGUE.permissions;
  for (const name of held) {
    const def = defs.find((entry) => entry.name === name);
    const step = def === undefined ? null : STEP[def.level];
    if (def === undefined || step === null) continue;
    for (const other of defs) {
      const otherStep = STEP[other.level];
      if (other.area === def.area && otherStep !== null && otherStep <= step) all.add(other.name);
    }
  }
  for (const def of defs) if (def.with !== null && all.has(def.with)) all.add(def.name);
  if (all.has('ports.manage')) all.add('ports.view');
  return all;
}

/** The rows of the kinds that can be made inside each kind of place. */
const INSIDE: Record<string, readonly string[]> = {
  org: ['org', 'project', 'topic', 'thread', 'environment', 'provider', 'agent', 'host', 'template', 'mcp_server', 'mount'],
  project: ['topic', 'thread', 'environment'],
  topic: ['thread', 'environment'],
};

/** What the system ships on its defaults page: the role that holds each permission, for work in a managed environment and on a person's machine. */
const SHIPPED: Record<string, { managed: Role | null; connected: Role | null }> = (() => {
  const same = (role: Role | null) => ({ managed: role, connected: role });
  const split = (managed: Role | null, connected: Role | null = null) => ({ managed, connected });
  return {
    'chat.view': same('viewer'), 'chat.interact': same('viewer'), 'chat.manage': same('developer'),
    'doc.view': same('viewer'), 'doc.edit': same('interactor'), 'doc.create': same('viewer'), 'doc.comment': same('viewer'), 'doc.manage': same('developer'),
    'thread.view': same('viewer'), 'thread.interact': split('interactor'), 'thread.manage': split('developer'), 'thread.fork': same('viewer'), 'thread.archive': same(null),
    'files.view': same('viewer'), 'files.manage': split('developer'), 'terminal.view': same('viewer'), 'terminal.manage': split('developer'),
    'topic.view': same('viewer'), 'topic.interact': same('developer'), 'topic.manage': same('developer'), 'topic.archive': same(null), 'topic.start_threads': same(null),
    'project.view': same('viewer'), 'project.interact': same('developer'), 'project.manage': same('admin'), 'project.archive': same(null), 'project.create_topics': same(null),
    'org.view': same('viewer'), 'org.interact': same('admin'), 'org.manage': same('admin'), 'org.archive': same(null),
    'settings.manage': same('admin'), 'usage.view': same('admin'), 'usage.summary': same('viewer'), 'usage.manage': same('admin'), 'usage.bypass': same('admin'),
    'audit.view': same('admin'), 'members.manage': same('admin'), 'directs.view': same('admin'),
    'access.manage': same(null), 'access.external': same('developer'),
    'thread.export': same('viewer'), 'chat.export': same('viewer'), 'doc.export': same('viewer'),
    'environment.view': same('viewer'), 'environment.use': split('interactor'), 'environment.manage': split('developer'),
    'browser.view': same('viewer'), 'browser.manage': same(null), 'vnc.view': same('viewer'), 'vnc.manage': same(null),
    'ports.view': same('viewer'), 'ports.manage': same(null),
  };
})();

export interface StoredRow {
  inherit: boolean;
  entries: Entry[];
  locked: boolean;
}

export interface StoredPage {
  /** `nodes`, or the path of a kind of resource. */
  path: string;
  id: string;
  defaults: boolean;
  kind: string;
  name: string;
  revision: number;
  owners: Entry[];
  rows: Map<string, StoredRow>;
}

interface Line {
  id: string;
  at: string;
  actor: UserRef;
  action: string;
  text: string;
}

interface Refused {
  permissions: string[];
}

/** A person who is in no org of the team's tree, for the tests of naming somebody from outside. */
export const OUTSIDER = { n: 10, username: 'zoe', name: 'Zoe Outsider', password: 'beyond the wall' } as const;

/** The paths of the pages beside the nodes', each of a kind of resource that the resources part keeps; their home is the team's org. */
const RESOURCES = { providers: 'provider', agents: 'agent', environments: 'environment', hosts: 'host', templates: 'template' } as const;
type ResourcePath = keyof typeof RESOURCES;
const isResource = (path: string): path is ResourcePath => path in RESOURCES;

const paged = <Item,>(items: readonly Item[], query: URLSearchParams): { items: Item[]; next_cursor: string | null } => {
  const limit = Math.min(200, Math.max(1, Number(query.get('limit') ?? 50)));
  const from = Number(query.get('cursor') ?? 0);
  const next = from + limit;
  return { items: items.slice(from, next), next_cursor: next < items.length ? String(next) : null };
};

export class Access implements Part {
  readonly world: World;
  /** What was shared with agents, and the policy and limits that decide it. */
  readonly shares: Shares = new Shares(this);
  /** What was asked for, the approvals it made, and the decisions. */
  readonly requests: Requests = new Requests(this);
  private pages = new Map<string, StoredPage>();
  private lines = new Map<string, Line[]>();
  private groups: { group: Group; members: string[]; holders?: HolderInput[] }[] = [];
  private resolvingRoles = new Set<string>();
  private resolvingViewers = new Set<string>();
  private explicitRoles: { role: RoleHeld; node: string }[] = [];
  private refusals = new Map<string, Refused>();
  /** Whether the previews and asking for access are served; the real system serves neither yet. */
  previewsServed = false;
  requestsServed = false;
  /** Whether owners may name somebody from outside (`sharing.allow_external`). */
  ownersMayShareOutside = true;
  /** People who may not name somebody from outside, whatever role they hold: what a place's policy and rows can bring about. */
  noOutsiders = new Set<string>();
  /** Threads that run on a person's machine. */
  connected = new Set<string>();
  private clock = 0;
  /** The provider is open to the work in these places. */
  private providerOpenTo = new Set<string>();
  /** Where the policy `sharing.allow_holders` was set, by place: true when holders may share; the nearest place on the way up decides, and none set means true, as shipped. */
  private allowHolders = new Map<string, boolean>();
  /** The changes of pages that were asked for and made, newest last, for a test to read what a form sent. */
  readonly sent: { path: string; id: string; change: PageChange }[] = [];

  /** Current direct node grants between two people; projection through both people's View is the card reader's job. */
  directShares(one: Person, other: Person): string[] {
    const between = (giver: string | undefined, recipient: string): boolean =>
      (giver === one.me.id && recipient === other.me.id) || (giver === other.me.id && recipient === one.me.id);
    const active = (expires: string | null): boolean => expires === null || Date.parse(expires) > Date.now();
    const ids = new Set<string>();
    for (const page of this.pages.values()) {
      if (page.path !== 'nodes' || page.defaults) continue;
      const entries = [...page.owners, ...[...page.rows.values()].flatMap((row) => row.entries)];
      if (entries.some((entry) => entry.holder.kind === 'user' && between(entry.created_by?.id, entry.holder.user.id) && active(entry.expires_at))) ids.add(page.id);
    }
    for (const { role, node } of this.explicitRoles) {
      if (role.subject.kind === 'user' && between(role.created_by.id, role.subject.id) && active(role.expires_at)) ids.add(node);
    }
    return [...ids].sort();
  }

  constructor(world: World) {
    this.world = world;
    this.reset();
  }

  reset(): void {
    this.pages.clear();
    this.lines.clear();
    this.explicitRoles = [];
    this.refusals.clear();
    this.previewsServed = true;
    this.requestsServed = true;
    this.ownersMayShareOutside = true;
    this.noOutsiders.clear();
    this.connected.clear();
    this.providerOpenTo.clear();
    this.allowHolders.clear();
    this.sent.length = 0;
    this.shares.reset();
    this.requests.reset();
    this.clock = 0;
    this.groups = [
      { group: { id: fixtureId(8, 1), name: 'Backend team', description: 'Who work on the services', member_count: 3, org_id: ACME.org }, members: ['grace', 'dora', 'linus'] },
      { group: { id: fixtureId(8, 2), name: 'Design', description: '', member_count: 2, org_id: ACME.org }, members: ['margaret', 'vera'] },
      { group: { id: fixtureId(0x30, 1), name: 'Operators', description: 'Who keeps the machines running', member_count: 2, org_id: ACME.org }, members: ['dora', 'grace'] },
      { group: { id: fixtureId(0x30, 2), name: 'Interns', description: "This season's newcomers", member_count: 1, org_id: ACME.org }, members: ['linus'] },
    ];
  }

  /** Makes the people the tests need beyond the team: somebody of no org of its tree. Called once the world has its people. */
  seed(): void {
    const template = this.person('ada');
    // A scenario with nobody in it, a system still to be set up, has no team to be outside of.
    if (template === undefined || this.world.people.has(OUTSIDER.username)) return;
    const me = { ...template.me, id: fixtureId(1, OUTSIDER.n), username: OUTSIDER.username, display_name: OUTSIDER.name, email: `${OUTSIDER.username}@example.org`, is_system_admin: false, personal_org_id: fixtureId(2, OUTSIDER.n), avatar: { text: 'Z', color: 4 } };
    this.world.addPerson(me, OUTSIDER.password);
  }

  // ---------------------------------------------------------------- people, places, kinds

  private person(username: string): Person | undefined {
    return this.world.people.get(username);
  }

  private groupOf(id: string): { group: Group; members: string[]; holders?: HolderInput[] } {
    const entry = this.groups.find((one) => one.group.id === id);
    if (entry === undefined) throw new Refusal(404, 'not_found', 'no such group');
    return entry;
  }

  private personById(id: string): Person | undefined {
    for (const person of this.world.people.values()) if (person.me.id === id) return person;
    return undefined;
  }

  private ref(person: Person): UserRef {
    return userRef(person.me);
  }

  private nodeRef(id: string): NodeRef | 'system' {
    if (id === 'system') return 'system';
    const row = this.world.places.pathTo(id).at(-1);
    return { id, kind: row?.kind ?? 'org', name: row?.name ?? null, redacted: row === undefined };
  }

  private kindOf(path: string, id: string): string {
    if (path === 'documents') return 'document';
    if (isResource(path)) return RESOURCES[path];
    return this.world.places.pathTo(id).at(-1)?.kind ?? 'org';
  }

  private nameOf(path: string, id: string): string {
    if (path === 'documents') return this.world.documents.docs.get(id)?.title ?? '';
    if (isResource(path)) return this.world.resources.describe(RESOURCES[path], id)?.name ?? '';
    return this.world.places.pathTo(id).at(-1)?.name ?? '';
  }

  /** The places above a node, nearest first, then the system. */
  private above(id: string): string[] {
    const ids = this.world.places.pathTo(id).map((row) => row.id);
    return [...ids.slice(0, -1).reverse(), 'system'];
  }

  private rootOf(path: string, id: string): string {
    if (path === 'documents') return this.world.places.rootOf(this.world.documents.docs.get(id)?.nodeId ?? '') ?? ACME.org;
    return isResource(path) ? ACME.org : (this.world.places.rootOf(id) ?? ACME.org);
  }

  private memberRole(username: string, root: string): Role | null {
    return this.world.places.roles.get(root)?.get(username)?.role ?? null;
  }

  private isMember(person: Person, root: string): boolean {
    return this.world.places.isMember(person, root);
  }

  // ---------------------------------------------------------------- for the shares with agents, which keep records of their own

  /** A node as the API refers to it. */
  nodeRefOf(id: string): NodeRef | 'system' {
    return this.nodeRef(id);
  }

  /** A person as the API refers to them. */
  userRefOf(person: Person): UserRef {
    return this.ref(person);
  }

  /** A selected person whose identity an access request may name. */
  personOfId(id: string): Person | undefined {
    return [...this.world.people.values()].find((person) => person.me.id === id);
  }

  /** The places above a node, nearest first, then the system. */
  ancestorsOf(id: string): string[] {
    return this.above(id);
  }

  /** The live page or the defaults page of a node (or of a resource, by its path), made as it is first asked for. */
  pageFor(id: string, defaults = false, path = 'nodes'): StoredPage {
    return this.pageOf(path, id, defaults);
  }

  /** A chat's disclosed inspection audience does not grant access to its org's permission page. */
  inspectionHolders(person: Person, org: string): Holder[] {
    const row = this.rowsFor(this.pageOf('nodes', org, false), person).rows.find((row) => row.permission === 'directs.view');
    return row === undefined ? [] : [...row.entries.map((entry) => entry.holder), ...(row.inherited?.holders ?? [])];
  }

  /** What a person holds on the page of a node (or of a resource, by its path), and whether they may share it. */
  holdingOn(person: Person, id: string, path = 'nodes'): { held: Set<string>; mayShare: boolean; manages: boolean } {
    const page = this.pageOf(path, id, false);
    const rows = this.rowsFor(page, person).rows;
    const held = this.heldOn(person, page, rows);
    // What a refusal names is what the person lacks, whatever the page's rows would say: a request for it is not for what they hold.
    for (const name of this.refusals.get(`${person.me.username}:${id}`)?.permissions ?? []) held.delete(name);
    // What the places were told to take from somebody is lacked here too: a permission taken everywhere, or on this very thing (an Export row, say), so that asking for it is asking for what they lack.
    for (const entry of this.world.places.denied.get(person.me.username) ?? []) {
      const at = entry.indexOf('@');
      if (at === -1) held.delete(entry);
      else if (entry.slice(at + 1) === id) held.delete(entry.slice(0, at));
    }
    return { held, mayShare: this.mayShare(person, page), manages: this.mayManage(person, page) };
  }

  /** Documents use their own rows once their page has been read or changed, rather than a separate fixed role table. */
  storedHoldingOn(person: Person, id: string, path: string): Set<string> | null {
    return this.pages.has(`${path}:${id}:live`) ? this.holdingOn(person, id, path).held : null;
  }

  /** What a thing is called, by the path of its kind. */
  objectName(path: string, id: string): string {
    return this.nameOf(path, id);
  }

  /** The refusal for something a person lacks on a node, as a change of a page words it. */
  lackingOn(id: string, permissions: string[]): Refusal {
    return this.lacking(this.pageOf('nodes', id, false), permissions);
  }

  /** The moment the fake world is at, advanced by a second. */
  now(): string {
    return this.stamp();
  }

  // ---------------------------------------------------------------- what a person holds

  /** An explicit ordinary assignment at this node, without expanding an inherited role into every child. */
  namedAt(person: Person, nodeId: string): boolean {
    return this.namedOn(person, 'nodes', nodeId);
  }

  /** The same direct user/group page facts for documents and resources. */
  namedOn(person: Person, path: string, objectId: string): boolean {
    const active = (expires: string | null): boolean => expires === null || Date.parse(expires) > Date.now();
    if (path === 'nodes' && this.explicitRoles.some((entry) => entry.node === objectId && active(entry.role.expires_at) && this.subjectIs(entry.role, person))) return true;
    const page = this.pages.get(`${path}:${objectId}:live`);
    if (page === undefined) return false;
    const place = this.placeOf(page);
    return [...page.owners, ...[...page.rows.values()].flatMap((row) => row.entries)].some((entry) =>
      active(entry.expires_at) && (entry.holder.kind === 'user' || entry.holder.kind === 'group') && this.isHolder(person, entry.holder, place));
  }

  /** The place whose roles a page is read by: the node itself, or for a resource the org it is at home in. */
  private placeOf(page: Pick<StoredPage, 'path' | 'id'>): string {
    if (page.path === 'documents') return this.world.documents.docs.get(page.id)?.nodeId ?? '';
    return isResource(page.path) ? ACME.org : page.id;
  }

  private subjectIs(held: RoleHeld, person: Person): boolean {
    switch (held.subject.kind) {
      case 'user':
        return held.subject.id === person.me.id;
      case 'group':
        return this.groupHas(held.subject.id, person);
      case 'members':
        return this.memberAt(person, held.subject.id);
      case 'role': {
        if (held.subject.role === 'viewer') return this.viewerAt(person, held.subject.id);
        const role = this.roleOf(person, held.subject.id);
        return role !== null && held.subject.role !== null && held.subject.role !== undefined && rank(role) >= rank(held.subject.role);
      }
    }
  }

  /** The highest role a person was given in a place on the way up from a node (the roles of their domain are not among them): the places read it to know what a role given there reaches. */
  givenRoleAlong(person: Person, nodeId: string): Role | null {
    const key = `${person.me.id}:${nodeId}`;
    if (this.resolvingRoles.has(key)) return null;
    this.resolvingRoles.add(key);
    try {
    let best: Role | null = null;
    for (const place of [nodeId, ...this.above(nodeId)]) {
      for (const entry of this.explicitRoles) {
        if (entry.node !== place || !this.subjectIs(entry.role, person)) continue;
        if (best === null || rank(entry.role.role) > rank(best)) best = entry.role.role;
      }
    }
    return best;
    } finally { this.resolvingRoles.delete(key); }
  }

  private memberAt(person: Person, org: string): boolean {
    const root = this.rootOf('nodes', org);
    const membership = this.world.places.roles.get(root)?.get(person.me.username);
    return membership !== undefined && this.world.places.pathTo(membership.domain).some((place) => place.id === org);
  }

  private viewerAt(person: Person, place: string): boolean {
    const key = `${person.me.id}:${place}`;
    if (this.resolvingViewers.has(key)) return false;
    this.resolvingViewers.add(key);
    try {
      const row = this.world.places.rowOf(place);
      return row !== undefined && this.world.places.access(person, place).permissions.includes(`${row.kind}.view`);
    } finally { this.resolvingViewers.delete(key); }
  }

  private groupHas(id: string, person: Person, seen = new Set<string>()): boolean {
    if (seen.has(id)) return false;
    const group = this.groups.find((group) => group.group.id === id);
    if (group === undefined) return false;
    const next = new Set(seen); next.add(id);
    return group.members.includes(person.me.username) || (group.holders ?? []).some((holder) => {
      if (holder.kind === 'user') return holder.id === person.me.id;
      if (holder.kind === 'group') return this.groupHas(holder.id, person, next);
      if (holder.kind === 'members') return this.memberAt(person, holder.id);
      if (holder.kind === 'role') { if (holder.role === 'viewer') return this.viewerAt(person, holder.at ?? group.group.org_id); const held = this.roleOf(person, holder.at ?? group.group.org_id); return held !== null && rank(held) >= rank(holder.role); }
      return false;
    });
  }

  private groupInputs(id: string): HolderInput[] {
    const group = this.groupOf(id);
    return [...group.members.flatMap((username) => { const person = this.person(username); return person === undefined ? [] : [{ kind: 'user' as const, id: person.me.id }]; }), ...(group.holders ?? [])];
  }

  private groupLabel(id: string): Group {
    const group = this.groupOf(id).group;
    return { ...group, place: this.world.places.pathTo(group.org_id).map((row) => row.name).join(' > '),
      member_count: [...this.world.people.values()].filter((person) => this.groupHas(id, person)).length };
  }

  /** A group's current public projection, admitted through the org that keeps it. */
  groupFor(person: Person, id: string): Group | undefined {
    const found = this.groups.find((entry) => entry.group.id === id);
    if (found === undefined) return undefined;
    this.world.places.need(person, found.group.org_id, 'org.view');
    return this.groupLabel(id);
  }

  private groupEdgeAllowed(outer: string, inner: string): boolean {
    const visit = (id: string, path: string[]): boolean => {
      if (path.includes(id) || path.length >= 5) return false;
      const children = this.groupInputs(id).flatMap((holder) => holder.kind === 'group' ? [holder.id] : []);
      if (id === outer && !children.includes(inner)) children.push(inner);
      return children.every((child) => visit(child, [...path, id]));
    };
    return this.groups.every((entry) => visit(entry.group.id, []));
  }

  /**
   * What a node's own page does to what a person holds there through their role: the entries that name them add what the rows say
   * (with the levels below), and a page whose View is cut loose from its place closes the node to everybody who is not named on it, owns
   * it or administers it. Read from the stored rows alone, so that the places can ask it while they work out what a person holds.
   */
  pageEffect(person: Person, nodeId: string): { adds: string[]; closed: boolean } {
    const page = this.pages.get(`nodes:${nodeId}:live`);
    if (page === undefined) return { adds: [], closed: false };
    const named = new Set<string>();
    for (const [key, row] of page.rows) {
      const permission = key.split('|')[0] ?? '';
      if (row.entries.some((entry) => this.isHolder(person, entry.holder, nodeId))) named.add(permission);
    }
    const owner = this.owns(person, page);
    const admin = this.roleOf(person, nodeId) === 'admin';
    const view = page.rows.get(`${page.kind}.view|any`);
    const closed = view !== undefined && !view.inherit && !owner && !admin && !named.has(`${page.kind}.view`);
    return { adds: owner ? [] : [...withLevels(named)], closed };
  }

  /** A broadcast follows inherited view rows below its origin, even when its sender owns a private branch. */
  announcementsFollow(node: string, origin: string): boolean {
    const way = this.world.places.pathTo(node);
    const start = way.findIndex((place) => place.id === origin);
    return start >= 0 && way.slice(start + 1).every((place) => !place.restricted && this.pages.get(`nodes:${place.id}:live`)?.rows.get(`${place.kind}.view|any`)?.inherit !== false);
  }

  /** The role a person holds where a node is: that of their domain or one given along the way, the higher; an admin of the system holds Admin everywhere. */
  roleOf(person: Person, nodeId: string): Role | null {
    return person.me.is_system_admin ? 'admin' : this.world.places.roleAt(person, nodeId);
  }

  /** How many people hold a role in a place, or a higher one: what a chip of the role says. */
  private countRole(nodeId: string, role: Role, includeViewers = false): number {
    let count = 0;
    for (const person of this.world.people.values()) {
      const held = this.roleOf(person, nodeId);
      if (role === 'viewer' && includeViewers ? this.viewerAt(person, nodeId) : held !== null && rank(held) >= rank(role)) count += 1;
    }
    return count;
  }

  /** A role as a holder of a row, with the head-count of the people it comes to now. */
  private roleHolder(role: Role, at: NodeRef | null, nodeId: string): Holder {
    const path = this.world.places.pathTo(at?.id ?? nodeId);
    const place = path.filter((row) => row.kind !== 'thread').map((row) => row.name).join(' > ');
    return { kind: 'role', role, at, place, count: this.countRole(at?.id ?? nodeId, role, at !== null) };
  }

  /** Whether the person owns a thing: they are named in its Owners row, or are in a group that is. */
  private owns(person: Person, page: StoredPage): boolean {
    return page.owners.some((entry) => {
      const holder = entry.holder;
      if (holder.kind === 'user') return holder.user.id === person.me.id;
      if (holder.kind !== 'group') return false;
      const id = holder.group.id;
      return this.groupHas(id, person);
    });
  }

  /** Whether a holder of a row is the person: a role they hold or a higher one where the thing is, a group they are in, themself, or everyone. */
  isHolder(person: Person, holder: Holder, nodeId: string): boolean {
    switch (holder.kind) {
      case 'role': {
        if (holder.role === 'viewer' && holder.at !== null) return this.viewerAt(person, holder.at.id);
        const held = this.roleOf(person, holder.at?.id ?? nodeId);
        return held !== null && rank(held) >= rank(holder.role);
      }
      case 'group':
        return this.groupHas(holder.group.id, person);
      case 'members':
        return this.memberAt(person, holder.node.id);
      case 'user':
        return holder.user.id === person.me.id;
      case 'everyone':
        return true;
      default:
        return false;
    }
  }

  /** The place whose policy `sharing.allow_holders` decides for a page, and what it says: the nearest place on the way up that set it, else true as shipped. */
  private sharingPolicy(page: StoredPage): { value: boolean; at: string | null } {
    const chain = isResource(page.path) ? [ACME.org] : [page.id, ...this.above(page.id)];
    for (const place of chain) {
      const set = this.allowHolders.get(place);
      if (set !== undefined) return {value:set,at:place};
    }
    return {value:true,at:null};
  }

  private holdersMayShare(page: StoredPage): boolean { return this.sharingPolicy(page).value; }

  /** What a person holds on a page: all of it for an owner (but who may name outsiders), else what the rows they stand in name, with the levels below and the rows that belong with them. */
  private heldOn(person: Person, page: StoredPage, rows: readonly PermissionRow[]): Set<string> {
    const held = new Set<string>();
    if (this.owns(person, page)) {
      for (const row of rows) if (row.permission !== 'access.external') held.add(row.permission);
      held.add('access.manage');
      return withLevels(held);
    }
    for (const row of rows) {
      const decided = row.locked !== null && !row.locked.here;
      const named = decided ? [] : row.entries.map((entry) => entry.holder);
      const followed = row.inherit || decided ? (row.inherited?.holders ?? []) : [];
      if ([...named, ...followed].some((holder) => this.isHolder(person, holder, this.placeOf(page)))) held.add(row.permission);
    }
    // The admins of the place hold the page's management without being named.
    if (!isResource(page.path) ? this.roleOf(person, page.id) === 'admin' : person.me.is_system_admin) held.add('access.manage');
    return withLevels(held);
  }

  /** Whether a person may share what they hold on a page: the thing's owners and whoever manages its page always, anybody else where the place lets holders share. */
  private mayShare(person: Person, page: StoredPage): boolean {
    return this.owns(person, page) || this.mayManage(person, page) || this.holdersMayShare(page);
  }

  // ---------------------------------------------------------------- pages as stored

  private specs(page: Pick<StoredPage, 'defaults' | 'kind'>): { permission: string; profile: RowProfile; def: PermissionDef }[] {
    const kinds = page.defaults ? (INSIDE[page.kind] ?? []) : [page.kind];
    const defs = CATALOGUE.permissions.filter((def) => def.level !== 'own' && def.on.some((kind) => kinds.includes(kind)));
    return defs.flatMap((def) => {
      const profiles: RowProfile[] = page.defaults && def.profiles ? ['managed', 'connected'] : ['any'];
      return profiles.map((profile) => ({ permission: def.name, profile, def }));
    });
  }

  private pageOf(path: string, id: string, defaults: boolean): StoredPage {
    const key = `${path}:${id}:${defaults ? 'defaults' : 'live'}`;
    let page = this.pages.get(key);
    if (page === undefined) {
      const kind = this.kindOf(path, id);
      page = { path, id, defaults, kind, name: this.nameOf(path, id), revision: 1, owners: [], rows: new Map() };
      for (const spec of this.specs(page)) page.rows.set(`${spec.permission}|${spec.profile}`, { inherit: true, entries: [], locked: false });
      if (!defaults) {
        const resource = isResource(path) ? this.world.resources.describe(RESOURCES[path], id) : null;
        const owner = isResource(path) ? this.person(resource?.owner ?? 'ada') : this.person(path === 'documents' ? this.world.documents.docs.get(id)?.owner ?? '' : this.world.places.metaOf(id).owner);
        if (owner !== undefined) page.owners.push(this.entry({ kind: 'user', user: this.ref(owner) }, owner, 'creation'));
        if (path === 'documents' && this.world.documents.docs.get(id)?.page === 'restricted') for (const row of page.rows.values()) row.inherit = false;
      }
      this.pages.set(key, page);
    }
    return page;
  }

  private entry(holder: Holder, by: Person, source: Entry['source'] = 'page', more: Partial<Entry> = {}): Entry {
    return {
      id: randomUUID(),
      holder,
      options: {},
      read_first: false,
      outside: false,
      expires_at: null,
      note: '',
      source,
      created_by: this.ref(by),
      created_at: new Date(Date.parse('2026-10-11T10:00:00.000Z') + this.clock * 1_000).toISOString(),
      revision: 1,
      ...more,
    };
  }

  private note(page: StoredPage, person: Person, action: string, text: string): void {
    this.clock += 1;
    const key = `${page.path}:${page.id}`;
    const list = this.lines.get(key) ?? [];
    list.unshift({ id: randomUUID(), at: new Date(Date.parse('2026-10-11T10:00:00.000Z') + this.clock * 1_000).toISOString(), actor: this.ref(person), action, text });
    this.lines.set(key, list);
  }

  // ---------------------------------------------------------------- what a row follows

  /** The defaults row a place keeps for a permission; a place that has stored none follows and names nobody. */
  private defaultsRow(place: string, permission: string, profile: RowProfile): StoredRow {
    const page = this.pageOf('nodes', place, true);
    const found = page.rows.get(`${permission}|${profile}`);
    return found ?? { inherit: true, entries: [], locked: false };
  }

  /** The topmost place, among those from `start` upwards, whose defaults row for the permission is locked. */
  private lockedAt(start: string, permission: string, profile: RowProfile): string | null {
    let found: string | null = null;
    for (const place of [start, ...this.above(start)]) {
      if (place === 'system') break;
      const row = this.pageOf('nodes', place, true).rows.get(`${permission}|${profile}`);
      if (row?.locked === true) found = place;
    }
    return found;
  }

  /** What the defaults of a place and of the places above it give to a row, with the nearest place that gives anything. */
  private resolve(start: string, permission: string, profile: RowProfile, forId: string): { holders: Holder[]; from: NodeOrSystem | null } {
    const holders: Holder[] = [];
    let from: NodeOrSystem | null = null;
    for (const place of [start, ...this.above(start)]) {
      if (place === 'system') {
        const role = SHIPPED[permission]?.[profile === 'connected' ? 'connected' : 'managed'] ?? null;
        if (role !== null) {
          holders.push(this.roleHolder(role, null, forId));
          from ??= 'system';
        }
        break;
      }
      const row = this.defaultsRow(place, permission, profile);
      if (row.entries.length > 0) {
        holders.push(...row.entries.map((entry) => (entry.holder.kind === 'role' ? this.roleHolder(entry.holder.role, entry.holder.at, forId) : entry.holder)));
        from ??= this.nodeRef(place);
      }
      if (!row.inherit) break;
    }
    return { holders, from };
  }

  /** The profile a thread's page follows, by where it runs. */
  private profileOf(page: StoredPage): RowProfile {
    if (page.kind === 'environment') return this.world.resources.environments.some((record) => record.id === page.id && record.machine !== null) ? 'connected' : 'managed';
    return page.kind === 'thread' && this.connected.has(page.id) ? 'connected' : 'managed';
  }

  /** The rows of a page as one reader sees them, with what the reader holds on them and whether they may share: all of the page that asking what somebody holds needs, and no more, since the rest of a thread's page asks that of everybody again. */
  private rowsFor(page: StoredPage, person: Person): { rows: PermissionRow[]; mayChange: boolean; held: Set<string>; sharing: boolean } {
    const parent = page.path === 'documents' ? this.placeOf(page) : page.defaults ? (this.above(page.id)[0] ?? 'system') : (isResource(page.path) ? ACME.org : (this.above(page.id)[0] ?? 'system'));
    const mayChange = this.mayManage(person, page);
    const rows: PermissionRow[] = [];
    for (const spec of this.specs(page)) {
      const stored = page.rows.get(`${spec.permission}|${spec.profile}`) ?? { inherit: true, entries: [], locked: false };
      // Only what depends on where work runs has two rows on a defaults page; a thing's page follows the one that fits where it runs.
      const profile: RowProfile = !spec.def.profiles ? 'any' : spec.profile === 'any' ? this.profileOf(page) : spec.profile;
      const lock = parent === 'system' ? null : this.lockedAt(parent, spec.permission, profile);
      const here = page.defaults && stored.locked;
      const decided = lock !== null;
      const start = decided ? lock : parent;
      const source = page.path === 'documents' && stored.inherit ? this.rowsFor(this.pageOf('nodes', parent, false), person).rows.find((row) => row.permission === spec.permission) : undefined;
      const resolved = source !== undefined ? { holders: [...source.entries.map((entry) => entry.holder), ...(source.inherit ? source.inherited?.holders ?? [] : [])], from: this.nodeRef(parent) }
        : stored.inherit || decided ? this.resolve(start, spec.permission, profile, this.placeOf(page)) : null;
      rows.push({
        permission: spec.permission,
        profile: spec.profile,
        inherit: stored.inherit,
        inherited: resolved === null ? null : { holders: resolved.holders, from: resolved.from },
        entries: stored.entries.map((entry) => (entry.holder.kind === 'role' ? { ...entry, holder: this.roleHolder(entry.holder.role, entry.holder.at, this.placeOf(page)) } : entry)),
        locked: decided ? { by: this.nodeRef(lock), here: false } : here ? { by: this.nodeRef(page.id), here: true } : null,
        writable: mayChange && !decided,
        can_give: false,
      });
    }
    // What the reader holds decides what they may give: each row they hold, where they may share at all, and not under a lock above.
    const held = this.heldOn(person, page, rows);
    const sharing = !page.defaults && this.mayShare(person, page);
    for (const row of rows) {
      const decidedAbove = row.locked !== null && !row.locked.here;
      // A defaults page is changed by whoever manages the place; a live page's row is given by whoever holds it and may share.
      row.can_give = page.defaults ? mayChange && !decidedAbove : sharing && held.has(row.permission) && !decidedAbove;
    }
    return { rows, mayChange, held, sharing };
  }

  private build(page: StoredPage, person: Person): PermissionPage {
    const { rows, mayChange, held, sharing } = this.rowsFor(page, person);
    const viewRow = rows.find((row) => row.permission === `${page.kind}.view`);
    const changed = rows.some((row) => !row.inherit || row.entries.length > 0);
    const root = this.rootOf(page.path, page.id);
    const admins = [...(this.world.places.roles.get(root)?.entries() ?? [])].filter(([, held]) => held.role === 'admin').map(([username]) => username);
    const everyAdmin = new Set([...admins, ...[...this.world.people.values()].filter((entry) => entry.me.is_system_admin).map((entry) => entry.me.username)]);
    const owner = page.owners.find((entry) => entry.holder.kind === 'user');
    const reaches = !page.defaults && page.kind === 'thread'
      ? this.reachesOf(page)
      : null;
    return {
      object: { kind: page.kind, id: page.id, name: page.name },
      page: page.defaults ? 'defaults' : 'live',
      state: viewRow !== undefined && !viewRow.inherit ? 'restricted' : changed ? 'custom' : 'follows',
      revision: page.revision,
      owner: owner?.holder.kind === 'user' ? owner.holder.user : null,
      owners: page.defaults ? [] : page.owners,
      owners_writable: !page.defaults && this.mayOwn(person, page),
      external: this.mayShareOutside(person, page),
      followers: page.defaults ? this.followers(page) : null,
      admins: { count: everyAdmin.size, of: (page.path === 'documents' ? [this.placeOf(page), ...this.above(this.placeOf(page))] : isResource(page.path) ? [ACME.org, 'system'] : this.above(page.id)).map((place) => this.nodeRef(place)) },
      depends: this.depends(page, person),
      reaches,
      covers: this.coversOf(person, page),
      gives: this.givesOf(person, page, rows, held, sharing, mayChange),
      rows,
    };
  }

  /** What the reader may give here: the choices whose every row they may give, on a place the roles up to their own, and to agents what they may do themself. */
  private givesOf(person: Person, page: StoredPage, rows: readonly PermissionRow[], held: ReadonlySet<string>, sharing: boolean, manages: boolean): PageGives {
    const holdersShare = this.holdersMayShare(page);
    // A defaults page is changed by whoever manages it, with the bound of what its pages' holders hold (not drawn by any form).
    if (page.defaults) return { choices: manages ? [...CHOICES] : [], roles: [], agents: [], holders_share: holdersShare };
    if (!sharing) return { choices: [], roles: [], agents: [], holders_share: holdersShare };
    const given = (permission: string) => rows.find((row) => row.permission === permission)?.can_give === true;
    const choices = CHOICES.filter((choice) => {
      const needed = (CATALOGUE.choices.find((entry) => entry.key === choice)?.permissions[page.kind] ?? []).filter((name) => rows.some((row) => row.permission === name));
      return needed.length > 0 && needed.every(given);
    });
    const own: Role | null = this.owns(person, page) ? 'admin' : this.roleOf(person, page.id);
    const roles = ['org', 'project', 'topic'].includes(page.kind) && own !== null ? ROLES.filter((role) => rank(role) <= rank(own)) : [];
    const mayAny = (names: readonly string[]) => names.some((name) => held.has(name));
    const agents: AgentLevel[] = [
      ...(mayAny(['thread.view', 'chat.view', 'doc.view']) ? (['view'] as const) : []),
      ...(mayAny(['thread.interact', 'chat.interact', 'doc.edit']) ? (['edit'] as const) : []),
      ...(held.has('doc.manage') ? (['manage'] as const) : []),
    ];
    return { choices, roles, agents: isResource(page.path) ? [] : agents, holders_share: holdersShare };
  }

  /** The places and threads and documents inside a place that follow it, and those that stay apart because they were made private; as far as the reader may see them. */
  private coversOf(person: Person, page: StoredPage): PageCovers | null {
    if (page.defaults || isResource(page.path) || !['org', 'project', 'topic'].includes(page.kind)) return null;
    const none = () => ({ places: 0, threads: 0, documents: 0 });
    const follows = none();
    const apart = none();
    const tree = this.world.trees.get(this.rootOf(page.path, page.id));
    for (const row of tree?.nodes.values() ?? []) {
      if (row.deleted || row.id === page.id || !this.world.places.pathTo(row.id).some((above) => above.id === page.id)) continue;
      if (!this.world.places.access(person, row.id).permissions.includes(`${row.kind}.view`)) continue;
      const away = this.apartFrom(row.id, page.id);
      const side = away ? apart : follows;
      if (row.kind === 'thread') side.threads += 1;
      else side.places += 1;
      // The documents are the documents double's: what is written beside this row, as far as the reader may see it.
      const written = this.world.documents.countsBeside(person, row.id);
      side.documents += away ? written.open + written.closed : written.open;
      if (!away) apart.documents += written.closed;
    }
    return { ...follows, apart };
  }

  /** Whether a thing inside a place was made private, or lies in something that was: its page's View row is cut loose, or the tree marks it restricted. */
  private apartFrom(nodeId: string, placeId: string): boolean {
    for (const row of this.world.places.pathTo(nodeId).slice(this.world.places.pathTo(placeId).length)) {
      if (row.restricted) return true;
      const view = this.pages.get(`nodes:${row.id}:live`)?.rows.get(`${row.kind}.view|any`);
      if (view !== undefined && !view.inherit) return true;
    }
    return false;
  }

  private followers(page: StoredPage): number {
    const own = this.world.places.pathTo(page.id).at(-1);
    return own === undefined ? 0 : 1 + this.world.places.pathTo(page.id).length;
  }

  /** What a thread runs on, as the resources part keeps it: the one thread's own, and for a topic that of the first thread in it (or what threads usually run on). */
  private runsOn(page: StoredPage): ThreadBinding {
    const resources = this.world.resources;
    if (page.kind === 'thread') return resources.bindingOf(page.id);
    const inside = [...(this.world.trees.get(this.rootOf(page.path, page.id))?.nodes.values() ?? [])].find((row) => row.kind === 'thread' && !row.deleted && this.world.places.pathTo(row.id).some((above) => above.id === page.id));
    return resources.bindingOf(inside?.id ?? '');
  }

  /** The page of the environment a thread runs in, with how it is referred to; `null` for a thread that runs in none. */
  private environmentOf(threadId: string): { page: StoredPage; ref: NonNullable<ReturnType<Resources['refsOf']>['environment']> } | null {
    const resources = this.world.resources;
    const ref = resources.refsOf(resources.bindingOf(threadId)).environment;
    return ref === null ? null : { page: this.pageOf('environments', ref.id, false), ref };
  }

  /** The permissions a person holds on the environment a thread runs in, sorted: what the views beside the conversation ask, since a thread's own page holds none of them. */
  environmentPermissions(person: Person, threadId: string): string[] {
    const found = this.environmentOf(threadId);
    return found === null ? [] : this.permissionsOnEnvironment(person, found.page.id);
  }

  /** The permissions a person holds on an environment, sorted: its views and its four doors, less what a test took from them. */
  permissionsOnEnvironment(person: Person, environmentId: string): string[] {
    const page = this.pageOf('environments', environmentId, false);
    const held = this.heldOn(person, page, this.rowsFor(page, person).rows);
    const taken = [...(this.world.places.denied.get(person.me.username) ?? [])];
    return ENVIRONMENT_PERMISSIONS.filter((name) => held.has(name) && !taken.some((entry) => entry === name || entry === `${name}@${environmentId}`));
  }

  /** The people who look into an environment: those who hold its `files.view`, which is how its files are read whatever any thread's page says. */
  private peopleLooking(page: StoredPage): Person[] {
    return [...this.world.people.values()].filter((person) => this.heldOn(person, page, this.rowsFor(page, person).rows).has('files.view'));
  }

  /** What reaching a thread's page reaches: the environment it works in, how many people look into it, and how many other threads work there. */
  private reachesOf(page: StoredPage): NonNullable<PermissionPage['reaches']> {
    const resources = this.world.resources;
    const binding = resources.bindingOf(page.id);
    const found = this.environmentOf(page.id);
    return {
      environment: { ...found!.ref, placement: this.connected.has(page.id) ? 'connected' : (found?.ref.placement ?? 'managed') },
      people: found === null ? 0 : this.peopleLooking(found.page).length,
      threads: Math.max(0, resources.reachOf(binding.environment).threads - 1),
    };
  }

  /** Who reaches into an environment, and how: through its own page (managing it, or the highest level of each door), as its owner, or by talking to an agent that works there. */
  private reachOf(person: Person, environmentId: string): { hidden: number; people: unknown[] } {
    const page = this.pageOf('environments', environmentId, false);
    this.must(person, page);
    const threads = [...this.world.resources.bindings].filter(([thread, binding]) => binding.environment === environmentId && this.world.places.rowOf(thread)?.deleted === false).map(([thread]) => thread);
    const people = [...this.world.people.values()].flatMap((subject) => {
      const held = this.heldOn(subject, page, this.rowsFor(page, subject).rows);
      const through: unknown[] = [];
      if (this.owns(subject, page)) through.push({ kind: 'owner' });
      if (held.has('environment.manage')) through.push({ kind: 'page', permission: 'environment.manage' });
      else {
        for (const door of ['files', 'terminal', 'browser', 'vnc']) {
          const level = held.has(`${door}.manage`) ? 'manage' : held.has(`${door}.view`) ? 'view' : null;
          if (level !== null) through.push({ kind: 'page', permission: `${door}.${level}` });
        }
      }
      for (const thread of threads) if (this.holdingOn(subject, thread).held.has('thread.interact')) through.push({ kind: 'thread', permission: 'thread.interact', thread: this.nodeRef(thread) });
      return through.length === 0 ? [] : [{ user: this.ref(subject), through }];
    });
    return { hidden: 0, people };
  }

  /** What a thread's page shows beside its permissions: the model it runs on, for a thread and a topic; and for a thread the environment it runs in, which offers to let the people named look into it. */
  private depends(page: StoredPage, person: Person): PermissionPage['depends'] {
    if (page.defaults || page.path !== 'nodes' || (page.kind !== 'thread' && page.kind !== 'topic')) return [];
    const found: PermissionPage['depends'] = [];
    const open = this.providerOpenTo.has(page.id) ? 'all' : 'none';
    const binding = this.runsOn(page);
    const provider = this.world.resources.refsOf(binding).provider;
    if (provider !== null) {
      const owner = this.person(this.world.resources.describe('provider', binding.provider)?.owner ?? 'ada') ?? this.person('ada')!;
      const mayGive = person.me.username === owner.me.username || person.me.is_system_admin;
      found.push({
        for: page.kind === 'thread' ? 'thread.interact' : 'topic.start_threads',
        permission: 'provider.use',
        choice: null,
        object: { id: provider.id, kind: 'provider', name: provider.name, owner: this.ref(owner) },
        open,
        options: { models: [binding.model], efforts: binding.effort === null ? [] : [binding.effort] },
        can_give: mayGive,
        through: open === 'all' ? this.nodeRef(page.id) as never : null,
      });
    }
    const environment = page.kind === 'thread' ? this.environmentOf(page.id) : null;
    if (environment !== null) {
      const owner = this.person(this.world.resources.describe('environment', environment.ref.id)?.owner ?? 'ada') ?? this.person('ada')!;
      const looking = ['environment.view', 'files.view', 'terminal.view', 'browser.view', 'vnc.view', 'ports.view'];
      const items: { for: string; permission: string; wanted: string[] }[] = [
        { for: 'thread.view', permission: 'environment.view', wanted: looking },
        { for: 'thread.manage', permission: 'files.manage', wanted: ['files.manage'] },
        { for: 'thread.manage', permission: 'terminal.manage', wanted: ['terminal.manage'] },
      ];
      const here = this.holdingOn(person, environment.ref.id, 'environments');
      for (const item of items) {
        // Whether the people who hold the thread's own row hold what is offered on the environment: all of them, some, or nobody but its owner.
        const others = [...this.world.people.values()].filter((subject) => !this.owns(subject, environment.page) && this.holdingOn(subject, page.id).held.has(item.for));
        const covered = others.filter((subject) => item.wanted.every((name) => this.heldOn(subject, environment.page, this.rowsFor(environment.page, subject).rows).has(name)));
        found.push({
          for: item.for,
          permission: item.permission,
          choice: item.for === 'thread.view' ? 'full_view' : null,
          object: { id: environment.ref.id, kind: 'environment', name: environment.ref.name, owner: this.ref(owner) },
          open: others.length === 0 || covered.length === others.length ? 'all' : covered.length > 0 ? 'some' : 'none',
          options: null,
          can_give: here.mayShare && item.wanted.every((name) => here.held.has(name)),
          through: null,
        });
      }
    }
    return found;
  }

  // ---------------------------------------------------------------- who may

  private mayManage(person: Person, page: StoredPage): boolean {
    if (page.path === 'documents') return this.owns(person, page) || this.roleOf(person, this.placeOf(page)) === 'admin';
    if (isResource(page.path)) return person.me.is_system_admin || person.me.username === (this.world.resources.describe(RESOURCES[page.path], page.id)?.owner ?? 'ada');
    const held = this.world.places.access(person, page.id).permissions;
    return held.includes('access.manage') || (!page.defaults && this.world.places.metaOf(page.id).owner === person.me.username);
  }

  private mayOwn(person: Person, page: StoredPage): boolean {
    return this.mayManage(person, page);
  }

  private mayShareOutside(person: Person, page: StoredPage): boolean {
    if (this.noOutsiders.has(person.me.username)) return false;
    if (isResource(page.path)) return this.mayManage(person, page);
    const root = this.rootOf(page.path, page.id);
    const role = this.memberRole(person.me.username, root);
    if (person.me.is_system_admin || (role !== null && rank(role) >= rank('developer'))) return true;
    return this.ownersMayShareOutside && this.world.places.metaOf(page.id).owner === person.me.username;
  }

  private must(person: Person, page: StoredPage): void {
    if (page.path === 'documents') {
      const document = this.world.documents.docs.get(page.id);
      if (document === undefined || document.deleted) throw new Refusal(404, 'not_found', 'no such document');
      if (!this.mayManage(person, page) && !this.heldOn(person, page, this.rowsFor(page, person).rows).has('doc.view')) throw new Refusal(404, 'not_found', 'no such document');
      return;
    }
    if (isResource(page.path)) {
      if (this.world.resources.describe(RESOURCES[page.path], page.id) === null) throw new Refusal(404, 'not_found', 'no such thing');
      return;
    }
    this.world.places.need(person, page.id, `${page.kind}.view`);
  }

  /** The settings of a node that decide what agents may read of what is beside it: the one that lets them read their neighbours, and the size a copy may have. */
  sharingRows(nodeId: string): unknown[] {
    return [this.shares.settingOf(nodeId), this.shares.limitSetting()];
  }

  // ---------------------------------------------------------------- the requests

  private searchHolders(person: Person, query: URLSearchParams): { items: HolderCandidate[] } {
    const text = (query.get('q') ?? '').trim().toLowerCase();
    const wantedRoot = query.get('org_id');
    const rows = [...this.world.trees.values()].flatMap((tree) => [...tree.nodes.values()])
      .filter((row) => row.kind !== 'thread' && !row.deleted && this.world.places.access(person, row.id).permissions.includes(`${row.kind}.view`));
    const known = (id: string) => rows.some((row) => row.id === id) || (this.world.places.rowOf(id)?.kind === 'org' && this.isMember(person, this.rootOf('nodes', id)));
    const addressOf = (id: string) => {
      const path = this.world.places.pathTo(id); let start = 0;
      path.forEach((row, index) => { if (!known(row.id)) start = index + 1; });
      return path.slice(start).map((row) => this.world.places.metaOf(row.id).slug ?? row.name.toLowerCase().replaceAll(' ', '-')).join('.');
    };
    const placeOf = (id: string) => this.world.places.pathTo(id).map((row) => known(row.id) ? row.name : 'a private place').join(' > ');
    const nodeOf = (id: string) => ({ ...(this.nodeRef(id) as NodeRef), name: placeOf(id) });
    const roleWords: Record<string, Role> = { viewer: 'viewer', viewers: 'viewer', '查看者': 'viewer', interactor: 'interactor', interactors: 'interactor', '参与者': 'interactor', developer: 'developer', developers: 'developer', '开发者': 'developer', admin: 'admin', admins: 'admin', '管理员': 'admin' };
    const members = text.startsWith('members of ') || text.startsWith('成员 ');
    const every = /^(every|everyone|all|所有人|全部) /.test(text);
    const cleaned = members ? text.replace(/^(members of|成员) /, '') : every ? text.replace(/^(every|everyone|all|所有人|全部) /, '') : text;
    const [path = '', ...words] = cleaned.split(' ');
    const tail = words.join(' ');
    const directRole = roleWords[path];
    let start = query.get('place_id');
    if (start !== null && this.world.places.rowOf(start)?.kind === 'thread') start = this.world.places.rowOf(start)?.parent_id ?? null;
    const exact = rows.filter((row) => addressOf(row.id) === path || addressOf(row.id).endsWith(`.${path}`));
    const listingPlaces = query.get('list_places') === 'true';
    const matched = cleaned === '' && listingPlaces ? rows.filter((row) => wantedRoot === null || this.rootOf('nodes', row.id) === wantedRoot)
      : cleaned === '' || directRole !== undefined ? rows.filter((row) => row.id === start)
      : exact.length > 0 ? exact : rows.filter((row) => addressOf(row.id).split('.').some((_, offset, parts) => parts.slice(offset).join('.').startsWith(path)) || row.name.toLowerCase().includes(path));
    const items: HolderCandidate[] = [];
    const add = (input: HolderInput, holder: Holder, place: string, locator: string, root: string) => items.push({ input, holder, place, locator, outside: wantedRoot !== null && root !== wantedRoot });
    for (const row of matched) {
      const at = nodeOf(row.id); const address = addressOf(row.id); const place = placeOf(row.id); const root = this.rootOf('nodes', row.id);
      if (listingPlaces) { add({ kind: 'role', role: 'viewer', at: row.id }, this.roleHolder('viewer', at, row.id), place, address, root); continue; }
      if (members) { if (row.kind === 'org') add({ kind: 'members', id: row.id }, { kind: 'members', node: at }, place, `members of ${address}`, root); continue; }
      const roles = cleaned === '' || (query.get('q') ?? '').endsWith(' ') ? ROLES : directRole !== undefined ? [directRole] : roleWords[tail] !== undefined ? [roleWords[tail]] : tail === '' ? ['viewer' as const] : ROLES.filter((role) => role.startsWith(tail));
      for (const role of roles) add({ kind: 'role', role, at: row.id }, this.roleHolder(role, at, row.id), place, `${address} ${role}`, root);
      if (cleaned === '' || (query.get('q') ?? '').endsWith(' ')) {
        if (row.kind === 'org') add({ kind: 'members', id: row.id }, { kind: 'members', node: at }, place, `members of ${address}`, root);
        if (query.get('include_work') === 'true') add({ kind: 'node', id: row.id }, { kind: 'node', node: at }, place, `work in ${address}`, root);
      }
    }
    if (!listingPlaces && !members && !every) {
      for (const entry of this.groups) {
        if (!rows.some((row) => row.id === entry.group.org_id)) continue;
        const name = entry.group.name.toLowerCase();
        if (cleaned === '' ? entry.group.org_id !== start : tail === '' ? !name.startsWith(path) : !matched.some((row) => row.id === entry.group.org_id) || !name.startsWith(tail)) continue;
        const place = placeOf(entry.group.org_id); const group = { ...entry.group, place, member_count: [...this.world.people.values()].filter((one) => this.groupHas(entry.group.id, one)).length };
        add({ kind: 'group', id: group.id }, { kind: 'group', group }, place, `${addressOf(group.org_id)} ${group.name}`, this.rootOf('nodes', group.org_id));
      }
      if (tail === '' && path !== '' && directRole === undefined) {
        const mail = path.includes('@') && !path.startsWith('@');
        const username = mail ? path : path.split('.').at(-1)?.replace(/^@/, '') ?? '';
        for (const one of this.world.people.values()) {
          const exactName = one.me.username.toLowerCase() === username || (one.me.email ?? '').toLowerCase() === username;
          if (!exactName && !(this.isMember(person, ACME.org) && this.isMember(one, ACME.org) && (one.me.username.toLowerCase().startsWith(username) || one.me.display_name.toLowerCase().includes(username)))) continue;
          const domain = this.world.places.roles.get(wantedRoot ?? ACME.org)?.get(one.me.username)?.domain;
          const address = domain === undefined ? `@${one.me.username}` : `${addressOf(domain)}.${one.me.username}`;
          if (!mail && path.includes('.') && address !== path && !address.endsWith(`.${path}`)) continue;
          add({ kind: 'user', id: one.me.id }, { kind: 'user', user: { ...this.ref(one), address } }, address, address, this.isMember(one, wantedRoot ?? ACME.org) ? wantedRoot ?? ACME.org : 'outside');
        }
      }
    }
    const seen = new Set<string>();
    return { items: items.filter((item) => { const key = JSON.stringify(item.input); if (seen.has(key)) return false; seen.add(key); return true; }).sort((one, other) => Number(one.outside) - Number(other.outside) || one.place.localeCompare(other.place)) };
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;
    if (route === 'GET /holders/search') return { status: 200, body: this.searchHolders(person, query) };

    if (route === 'GET /permissions') return { status: 200, body: CATALOGUE };
    if ((found = /^POST \/(nodes|providers|agents|environments|hosts|templates)\/([^/]+)\/share\/preview$/.exec(route))) {
      return {status:200,body:this.sharePreview(person,found[1] ?? 'nodes',found[2] ?? '',body as unknown as SharePreviewBody)};
    }

    const shared = this.shares.handle(person, method, path, query, body);
    if (shared !== null) return shared;
    if ((found = /^PUT \/nodes\/([^/]+)\/settings\/sharing\.snapshot_max_bytes$/.exec(route))) {
      this.shares.control('/snapshot', { limit: Number(body.value) });
      return { status: 200, body: this.shares.limitSetting() };
    }

    if ((found = /^GET \/nodes\/([^/]+)$/.exec(route))) {
      const refused = this.refusals.get(`${person.me.username}:${found[1] ?? ''}`);
      if (refused !== undefined) this.refuse(person, found[1] ?? '', refused);
      return null;
    }

    if ((found = /^GET \/environments\/([^/]+)\/reach$/.exec(route))) return { status: 200, body: this.reachOf(person, found[1] ?? '') };

    if ((found = /^(GET|PATCH) \/(nodes|documents|providers|agents|environments|hosts|templates)\/([^/]+)\/permissions$/.exec(route))) {
      const page = this.pageOf(found[2] ?? 'nodes', found[3] ?? '', false);
      this.must(person, page);
      const answer = found[1] === 'GET' ? this.build(page, person) : this.change(person, page, body);
      if (found[1] === 'PATCH' && page.path === 'documents') {
        const document = this.world.documents.docs.get(page.id);
        if (document !== undefined) {
          const before = document.seq;
          document.seq += 1;
          document.page = page.rows.get('doc.view|any')?.inherit === false ? 'restricted' : 'follows';
          this.world.documents.permissionsChanged(page.id, before);
        }
      }
      return { status: 200, body: answer };
    }
    if ((found = /^(GET|PATCH) \/nodes\/([^/]+)\/defaults$/.exec(route))) {
      const page = this.pageOf('nodes', found[2] ?? '', true);
      this.must(person, page);
      return { status: 200, body: found[1] === 'GET' ? this.build(page, person) : this.change(person, page, body) };
    }
    if ((found = /^POST \/(nodes|documents|providers|agents|environments|hosts|templates)\/([^/]+)\/permissions\/preview$/.exec(route))) {
      return { status: 200, body: this.preview(person, this.pageOf(found[1] ?? 'nodes', found[2] ?? '', false), body, found[1] ?? 'nodes') };
    }
    if ((found = /^POST \/nodes\/([^/]+)\/defaults\/preview$/.exec(route))) {
      return { status: 200, body: this.preview(person, this.pageOf('nodes', found[1] ?? '', true), body, 'nodes') };
    }
    if ((found = /^GET \/(nodes|documents|providers|agents|environments|hosts|machines|templates|mcp-servers|mounts)\/([^/]+)\/permissions\/(effective|people)$/.exec(route))) {
      const page = this.pageOf(found[1] ?? 'nodes', found[2] ?? '', false);
      this.must(person, page);
      const user = query.get('user_id') ?? '';
      if (found[3] === 'effective' && user !== person.me.id && !this.mayManage(person, page)) throw new Refusal(403, 'authz.denied', 'Managing this permission page is required to inspect another person.');
      return { status: 200, body: found[3] === 'effective' ? this.effective(page, user) : this.people(page, query) };
    }
    if ((found = /^GET \/(nodes|providers|agents|environments|hosts|templates)\/([^/]+)\/history$/.exec(route))) {
      if (found[1] !== 'nodes') throw new Refusal(500, 'internal', 'something went wrong; the log has the cause');
      const page = this.pageOf('nodes', found[2] ?? '', false);
      this.must(person, page);
      const created: Line = { id: fixtureId(11, 1), at: '2026-10-01T09:00:00.000Z', actor: this.ref(this.person('ada')!), action: 'node.created', text: `Ada Lovelace made the ${page.kind} ${page.name}` };
      const actor = query.get('actor');
      // Native node_history applies its actor UUID before the cursor and limit.
      const lines = [...(this.lines.get(`nodes:${page.id}`) ?? []), created].filter((line) => actor === null || line.actor.id === actor);
      return { status: 200, body: paged(lines.map((line) => ({ ...line, about: null })), query) };
    }

    if ((found = /^GET \/nodes\/([^/]+)\/roles$/.exec(route))) return { status: 200, body: this.roleList(person, found[1] ?? '') };
    if ((found = /^POST \/nodes\/([^/]+)\/roles$/.exec(route))) return { status: 200, body: this.giveRole(person, found[1] ?? '', body) };
    if ((found = /^(PATCH|DELETE) \/roles\/([^/]+)$/.exec(route))) {
      const mine = this.explicitRoles.find((entry) => entry.role.id === found?.[2]);
      if (mine === undefined) return null;
      return this.editRole(person, mine, found[1] === 'DELETE', body);
    }
    if ((found = /^GET \/orgs\/([^/]+)\/groups$/.exec(route))) {
      const orgId = found[1] ?? '';
      this.world.places.need(person, orgId, 'org.view');
      // `under`: with the groups of the orgs inside it as well.
      const under = query.get('under') === 'true';
      const listed = this.groups.filter(({ group }) => group.org_id === orgId || (under && this.world.places.pathTo(group.org_id).some((row) => row.id === orgId)));
      return { status: 200, body: listed.map((entry) => this.groupLabel(entry.group.id)) };
    }
    if ((found = /^GET \/nodes\/([^/]+)\/groups$/.exec(route))) {
      const id = found[1] ?? ''; const row = this.world.places.rowOf(id);
      this.world.places.need(person, id, `${row?.kind ?? 'org'}.view`);
      const org = [...this.world.places.pathTo(id)].reverse().find((place) => place.kind === 'org')?.id;
      return { status: 200, body: this.groups.filter((entry) => entry.group.org_id === org).map((entry) => this.groupLabel(entry.group.id)) };
    }
    if ((found = /^(GET|PUT|DELETE) \/groups\/([^/]+)\/holders$/.exec(route))) {
      const id = found[2] ?? ''; const group = this.groupOf(id);
      this.world.places.need(person, group.group.org_id, found[1] === 'GET' ? 'org.view' : 'members.manage');
      const page = this.pageOf('nodes', group.group.org_id, false);
      if (found[1] === 'GET') return { status: 200, body: this.groupInputs(id).map((input) => this.holderOf(input, page).holder) };
      if (!['user', 'group', 'role', 'members'].includes(String(body.kind))) throw new Refusal(400, 'request.invalid', 'a group contains people, groups, roles or org members');
      const input = body as unknown as HolderInput;
      this.holderOf(input, page);
      if (input.kind === 'group' && found[1] === 'PUT' && !this.groupEdgeAllowed(id, input.id)) throw new Refusal(400, 'request.invalid', 'group nesting cannot contain a cycle or be deeper than five groups');
      if (input.kind === 'user') {
        const target = this.personById(input.id);
        if (target === undefined) throw new Refusal(404, 'not_found', 'no such person');
        group.members = group.members.filter((username) => username !== target.me.username);
        if (found[1] === 'PUT') group.members.push(target.me.username);
      } else {
        const stored: HolderInput = input.kind === 'role' ? { ...input, at: input.at ?? group.group.org_id } : input;
        group.holders = (group.holders ?? []).filter((holder) => JSON.stringify(holder) !== JSON.stringify(stored));
        if (found[1] === 'PUT') group.holders.push(stored);
      }
      return { status: 204 };
    }
    if ((found = /^POST \/orgs\/([^/]+)\/groups$/.exec(route))) {
      const orgId = found[1] ?? '';
      this.world.places.need(person, orgId, 'members.manage');
      const name = typeof body.name === 'string' ? body.name.trim() : '';
      if (name === '') throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path: 'name', message: 'must not be empty' }] });
      const made: Group = { id: randomUUID(), org_id: orgId, name, description: typeof body.description === 'string' ? body.description : '', member_count: 0 };
      this.groups.push({ group: made, members: [] });
      return { status: 200, body: made };
    }
    if ((found = /^(PATCH|DELETE) \/groups\/([^/]+)$/.exec(route))) {
      const entry = this.groupOf(found[2] ?? '');
      this.world.places.need(person, entry.group.org_id, 'members.manage');
      if (found[1] === 'DELETE') {
        this.groups = this.groups.filter((one) => one !== entry);
        return { status: 204 };
      }
      if (typeof body.name === 'string' && body.name.trim() !== '') entry.group.name = body.name.trim();
      if (typeof body.description === 'string') entry.group.description = body.description;
      return { status: 200, body: this.groupLabel(entry.group.id) };
    }
    if ((found = /^GET \/groups\/([^/]+)\/members$/.exec(route))) {
      const entry = this.groupOf(found[1] ?? '');
      this.world.places.need(person, entry.group.org_id, 'org.view');
      return { status: 200, body: entry.members.map((username) => this.ref(this.person(username)!)) };
    }
    if ((found = /^(PUT|DELETE) \/groups\/([^/]+)\/members\/([^/]+)$/.exec(route))) {
      const entry = this.groupOf(found[2] ?? '');
      this.world.places.need(person, entry.group.org_id, 'members.manage');
      const target = this.personById(found[3] ?? '');
      if (target === undefined) throw new Refusal(404, 'not_found', 'no such person');
      const others = entry.members.filter((username) => username !== target.me.username);
      entry.members = found[1] === 'PUT' ? [...others, target.me.username] : others;
      return { status: 204 };
    }

    if (route === 'POST /invitations' && body.kind === 'node') return { status: 200, body: this.inviteToPlace(person, body) };

    if (route === 'POST /access-requests' || /^GET \/access-requests\/[^/]+$/.test(route)) {
      if (!this.requestsServed) return this.unserved();
      return this.requests.handle(person, method, path, query, body);
    }
    // The approvals of access are this part's; the rest are the threads'.
    if (/^(GET|POST) \/approvals(\/|$)/.test(route)) return this.requests.handle(person, method, path, query, body);
    return null;
  }

  private unserved(): never {
    throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
  }

  /** The refusal a person gets for a node the test closed to them: what is missing, whom to ask, and whether they may force it. */
  private refuse(person: Person, nodeId: string, refused: Refused): never {
    const row = this.world.places.pathTo(nodeId).at(-1);
    const ownerName = this.world.places.metaOf(nodeId).owner;
    const owner = this.ref(this.person(ownerName)!);
    const root = this.rootOf('nodes', nodeId);
    const administers = person.me.is_system_admin || this.memberRole(person.me.username, root) === 'admin';
    const object = { kind: row?.kind ?? 'org', id: nodeId };
    throw new Refusal(403, 'authz.denied', `${refused.permissions[0] ?? 'node.view'} is required on this ${object.kind}`, {
      required: refused.permissions[0] ?? null,
      object,
      requestable: true,
      override: administers,
      missing: [{ object: { ...object, name: row?.name ?? null, owner }, permissions: refused.permissions, options: null, ask: [owner], ask_more: 0, override: administers, closed: null }],
    });
  }

  // ---------------------------------------------------------------- changes

  private holderOf(input: HolderInput, page: StoredPage): { holder: Holder; outside: boolean } {
    const root = this.rootOf(page.path, page.id);
    switch (input.kind) {
      case 'role':
        return { holder: this.roleHolder(input.role, input.at === undefined || input.at === null ? null : (this.nodeRef(input.at) as NodeRef), page.id), outside: false };
      case 'group': {
        const group = this.groups.find((entry) => entry.group.id === input.id);
        if (group === undefined) throw new Refusal(400, 'request.invalid', 'no such group', { errors: [{ path: 'holders', message: 'no such group' }] });
        return { holder: { kind: 'group', group: this.groupLabel(group.group.id) }, outside: false };
      }
      case 'user': {
        const person = this.personById(input.id);
        if (person === undefined) throw new Refusal(400, 'request.invalid', 'no such person', { errors: [{ path: 'holders', message: 'no such person' }] });
        return { holder: { kind: 'user', user: this.ref(person) }, outside: !this.isMember(person, root) };
      }
      case 'node':
        return { holder: { kind: 'node', node: this.nodeRef(input.id) as NodeRef }, outside: false };
      case 'members':
        return { holder: { kind: 'members', node: this.nodeRef(input.id) as NodeRef }, outside: false };
      case 'agents':
        return { holder: { kind: 'agents', node: this.nodeRef(input.id) as NodeRef }, outside: false };
      case 'everyone':
        return { holder: { kind: 'everyone' }, outside: false };
    }
  }

  /** What a refusal for a change says is missing, as the real system words it: the permissions, who could give them, and nothing to force. */
  private lacking(page: StoredPage, permissions: string[]): Refusal {
    const ada = this.ref(this.person('ada')!);
    return new Refusal(403, 'authz.denied', `${permissions[0] ?? 'access.manage'} is required`, {
      required: permissions[0] ?? 'access.manage',
      object: { kind: page.kind, id: page.id },
      requestable: true,
      override: false,
      missing: [{ object: { kind: page.kind, id: page.id, name: page.name, owner: ada }, permissions, options: null, ask: [ada], ask_more: 0, override: false, closed: null }],
    });
  }

  /**
   * Refuses a change its caller may not make. Whoever manages the page may change it as they like. A sharer
   * only names holders, in rows they hold and so up to what they hold, and takes back entries they made
   * themself; anything else takes managing the page, and a place that keeps sharing to owners lets a holder
   * do none of it.
   */
  private mayChangeBy(person: Person, page: StoredPage, wanted: PageChange): void {
    if (this.mayManage(person, page)) return;
    const built = this.build(page, person);
    const givable = (permission: string) => built.rows.some((row) => row.permission === permission && row.can_give);
    const others = wanted.owners !== undefined && wanted.owners !== null ? true : wanted.whole !== undefined && wanted.whole !== null;
    if (page.defaults || others || !this.mayShare(person, page)) throw this.lacking(page, ['access.manage']);
    for (const request of wanted.name ?? []) {
      const needed = (CATALOGUE.choices.find((choice) => choice.key === request.choice)?.permissions[page.kind] ?? []).filter((name) => built.rows.some((row) => row.permission === name));
      const missing = needed.filter((name) => !givable(name));
      if (missing.length > 0) throw this.lacking(page, missing);
    }
    for (const request of wanted.rows ?? []) {
      const changesRow = request.inherit !== undefined || request.locked !== undefined || (request.update?.length ?? 0) > 0;
      if (changesRow) throw this.lacking(page, ['access.manage']);
      if ((request.add?.length ?? 0) > 0 && !givable(request.permission)) throw this.lacking(page, [request.permission]);
      const row = built.rows.find((entry) => entry.permission === request.permission);
      // A sharer takes back what they gave, for as long as they could give it again.
      for (const id of request.remove ?? []) {
        const entry = row?.entries.find((candidate) => candidate.id === id);
        if (entry?.created_by?.id !== person.me.id || row?.can_give !== true) throw this.lacking(page, ['access.manage']);
      }
    }
  }

  private change(person: Person, page: StoredPage, change: Record<string, unknown>): PermissionPage {
    const wanted = change as PageChange;
    this.mayChangeBy(person, page, wanted);
    if (wanted.expected_revision !== undefined && wanted.expected_revision !== null && wanted.expected_revision !== page.revision) {
      throw new Refusal(409, 'conflict.stale_revision', 'the page was changed meanwhile', { current: this.build(page, person) });
    }
    this.sent.push({ path: page.path, id: page.id, change: wanted });
    this.apply(person, page, wanted);
    return this.build(page, person);
  }

  /** Applies a change to a stored page, and records it; a test also applies one as somebody else. */
  apply(person: Person, page: StoredPage, wanted: PageChange): void {
    const touched: string[] = [];
    const external = this.mayShareOutside(person, page);

    const named = (inputs: readonly HolderInput[]) =>
      inputs.map((input) => {
        const found = this.holderOf(input, page);
        if (found.outside && !external) {
          throw new Refusal(403, 'sharing.external_forbidden', 'naming somebody from outside takes the right to', {});
        }
        return found;
      });

    for (const request of wanted.name ?? []) {
      const holders = named(request.holders);
      const kind = page.kind;
      const covered = (CATALOGUE.choices.find((choice) => choice.key === request.choice)?.permissions[kind] ?? []).filter((name) => page.rows.has(`${name}|any`));
      // Export travels with the choice: the holders are named in the Export rows of the same areas, unless the giver left it out or may not give them.
      const requested = (request as { export?: boolean | null }).export;
      const exportable = requested === false ? [] : [...new Set(covered.map((name) => name.split('.')[0] ?? ''))].map((area) => `${area}.export`).filter((name) => page.rows.has(`${name}|any`) && this.heldOn(person, page, this.build(page, person).rows).has(name));
      const permissions = [...covered, ...exportable];
      for (const permission of permissions) {
        const row = page.rows.get(`${permission}|any`);
        if (row === undefined) continue;
        for (const found of holders) {
          if (row.entries.some((entry) => JSON.stringify(entry.holder) === JSON.stringify(found.holder))) continue;
          row.entries.push(this.entry(found.holder, person, 'page', { outside: found.outside, expires_at: request.expires_at ?? null, note: request.note ?? '' }));
        }
      }
      touched.push(`named ${holders.length} in ${request.choice}`);
      if (holders.some((found) => found.holder.kind === 'user' && found.holder.user.id === person.me.id)) this.refusals.delete(`${person.me.username}:${page.id}`);
    }

    for (const request of wanted.rows ?? []) {
      const profile: RowProfile = request.profile ?? 'any';
      const row = page.rows.get(`${request.permission}|${profile}`);
      if (row === undefined) throw new Refusal(400, 'request.invalid', 'no such row', { errors: [{ path: 'rows', message: `${request.permission} is not on this page` }] });
      const lock = this.lockedAt(this.above(page.id)[0] ?? 'system', request.permission, CATALOGUE.permissions.find((def) => def.name === request.permission)?.profiles === true && profile === 'any' ? this.profileOf(page) : profile);
      if (lock !== null) throw new Refusal(409, 'access.locked', `${this.nodeName(lock)} decides this`, { by: this.nodeRef(lock) });
      for (const add of request.add ?? []) {
        const [found] = named([add.holder]);
        if (found !== undefined) row.entries.push(this.entry(found.holder, person, 'page', { outside: found.outside, expires_at: add.expires_at ?? null, options: add.options ?? {}, note: add.note ?? '' }));
        if (page.kind === 'provider' && request.permission === 'provider.use' && add.holder.kind === 'node') this.providerOpenTo.add(add.holder.id);
      }
      if (request.remove !== undefined) row.entries = row.entries.filter((entry) => !request.remove?.includes(entry.id));
      for (const update of request.update ?? []) {
        const entry = row.entries.find((candidate) => candidate.id === update.entry_id);
        if (entry === undefined) continue;
        if (update.expires_at !== undefined) entry.expires_at = update.expires_at;
        if (update.options !== undefined) entry.options = update.options;
        if (update.note !== undefined && update.note !== null) entry.note = update.note;
        entry.revision += 1;
      }
      if (request.inherit !== undefined && request.inherit !== null) row.inherit = request.inherit;
      if (request.locked !== undefined && request.locked !== null) row.locked = request.locked;
      touched.push(request.permission);
    }

    if (wanted.whole === 'private') {
      for (const row of page.rows.values()) row.inherit = false;
      touched.push('private');
    } else if (wanted.whole === 'reset') {
      for (const row of page.rows.values()) {
        row.inherit = true;
        row.entries = [];
      }
      touched.push('reset');
    }

    if (wanted.owners !== undefined && wanted.owners !== null) {
      if (!this.mayOwn(person, page)) throw new Refusal(403, 'authz.denied', 'only an owner or an admin names owners', { required: 'access.own' });
      const owners = wanted.owners;
      for (const input of owners.add ?? []) {
        const [found] = named([input]);
        if (found !== undefined && !page.owners.some((entry) => JSON.stringify(entry.holder) === JSON.stringify(found.holder))) page.owners.push(this.entry(found.holder, person, 'page'));
      }
      if (owners.remove !== undefined) {
        const remaining = page.owners.filter((entry) => !owners.remove?.includes(entry.id));
        if (!remaining.some((entry) => entry.holder.kind === 'user')) throw new Refusal(409, 'owner.last', 'the last owner cannot be taken out');
        page.owners = remaining;
      }
      if (owners.first !== undefined && owners.first !== null) {
        const first = page.owners.find((entry) => entry.holder.kind === 'user' && entry.holder.user.id === owners.first);
        if (first !== undefined) page.owners = [first, ...page.owners.filter((entry) => entry !== first)];
      }
      touched.push('owners');
    }

    if (touched.length > 0) {
      page.revision += 1;
      const administers = person.me.is_system_admin || this.memberRole(person.me.username, this.rootOf(page.path, page.id)) === 'admin';
      this.note(page, person, 'permission.changed', `${person.me.display_name} changed the page${administers && !this.mayHold(person, page) ? ' (an override)' : ''}`);
    }
  }

  private mayHold(person: Person, page: StoredPage): boolean {
    return page.path === 'nodes' ? this.world.places.metaOf(page.id).owner === person.me.username : true;
  }

  private nodeName(id: string): string {
    return this.world.places.pathTo(id).at(-1)?.name ?? id;
  }

  // ---------------------------------------------------------------- previews, effective access, people

  private people(page: StoredPage, query: URLSearchParams): unknown {
    const holder = query.get('holder') ?? '';
    const root = this.rootOf(page.path, page.id);
    const members: ReadonlyMap<string, { role: Role }> = this.world.places.roles.get(root) ?? new Map<string, { role: Role }>();
    let usernames: string[];
    const [kind, first, second] = holder.split(':');
    if (kind === 'role') {
      const role = (second ?? first) as Role;
      // The people behind the chip are those its head-count counts: whoever holds the role where the chip names, or a higher one.
      const place = first === undefined || first === 'local' || first === 'system' || second === undefined ? page.id : first;
      usernames = [...this.world.people.values()].filter((entry) => {
        if (role === 'viewer' && first !== 'local') return this.viewerAt(entry, place);
        const held = this.roleOf(entry, place);
        return held !== null && rank(held) >= rank(role);
      }).map((entry) => entry.me.username);
      if (first === 'system') usernames = [...this.world.people.values()].filter((entry) => entry.me.is_system_admin).map((entry) => entry.me.username);
    } else if (kind === 'group') {
      usernames = [...this.world.people.values()].filter((person) => this.groupHas(first ?? '', person)).map((person) => person.me.username);
    } else if (kind === 'admins') {
      usernames = [...members.entries()].filter(([, held]) => held.role === 'admin').map(([username]) => username);
      for (const entry of this.world.people.values()) if (entry.me.is_system_admin && !usernames.includes(entry.me.username)) usernames.push(entry.me.username);
    } else if (kind === 'members' || kind === 'node') {
      usernames = [...this.world.people.values()].filter((person) => this.memberAt(person, first ?? page.id)).map((person) => person.me.username);
    } else {
      throw new Refusal(404, 'not_found', 'no such chip');
    }
    const refs = usernames.flatMap((username) => this.person(username) ?? []).map((entry) => this.ref(entry)).sort((one, other) => one.display_name.localeCompare(other.display_name));
    const listed = paged(refs, query);
    const owners = new Set([...this.world.people.values()].filter((person) => this.owns(person, page)).map((person) => person.me.id));
    return { ...listed, owners: listed.items.filter((user) => owners.has(user.id)).map((user) => user.id) };
  }

  private effective(page: StoredPage, userId: string): unknown {
    const subject = this.personById(userId);
    if (subject === undefined) throw new Refusal(404, 'not_found', 'no such person');
    const built = this.build(page, subject);
    const role = this.memberRole(subject.me.username, this.rootOf(page.path, page.id));
    const owns = page.owners.some((entry) => entry.holder.kind === 'user' && entry.holder.user.id === userId);
    const permissions: { permission: string; through: unknown[] }[] = [];
    for (const row of built.rows) {
      const through: unknown[] = [];
      if (owns) through.push({ kind: 'owner' });
      for (const entry of row.entries) if ((entry.holder.kind === 'user' && entry.holder.user.id === userId) || (entry.holder.kind === 'role' && role !== null && rank(role) >= rank(entry.holder.role))) through.push({ kind: 'entry', entry, from: null });
      for (const holder of row.inherited?.holders ?? []) if (holder.kind === 'role' && role !== null && rank(role) >= rank(holder.role)) through.push({ kind: 'entry', entry: this.entry(holder, subject, 'seed'), from: row.inherited?.from ?? 'system' });
      if (through.length > 0) permissions.push({ permission: row.permission, through });
    }
    return { user: this.ref(subject), role, administers: role === 'admin' || subject.me.is_system_admin, permissions };
  }

  private preview(person: Person, page: StoredPage, change: Record<string, unknown>, path: string): PagePreview {
    if (!this.previewsServed) throw new Refusal(404, 'not_found', 'there is no such thing, or it was deleted');
    this.must(person, page);
    const wanted = change as PageChange;
    const root = this.rootOf(path, page.id);
    const members: ReadonlyMap<string, { role: Role }> = this.world.places.roles.get(root) ?? new Map<string, { role: Role }>();
    const losing = new Map<string, Set<string>>();
    const lose = (username: string, permission: string) => {
      const set = losing.get(username) ?? new Set<string>();
      set.add(permission);
      losing.set(username, set);
    };
    const built = this.build(page, person);
    const rowsOf = (permission: string) => built.rows.filter((row) => row.permission === permission);
    const peopleOf = (holder: Holder): string[] =>
      holder.kind === 'user'
        ? [holder.user.username]
        : holder.kind === 'role'
          ? [...members.entries()].filter(([, held]) => rank(held.role) >= rank(holder.role)).map(([username]) => username)
          : holder.kind === 'group'
            ? (this.groups.find((entry) => entry.group.id === holder.group.id)?.members ?? [])
            : [];
    const owners = new Set(page.owners.flatMap((entry) => (entry.holder.kind === 'user' ? [entry.holder.user.username] : [])));
    if (wanted.whole === 'private') {
      for (const row of built.rows) for (const holder of row.inherited?.holders ?? []) for (const name of peopleOf(holder)) if (!owners.has(name)) lose(name, row.permission);
    }
    if (wanted.whole === 'reset') {
      for (const row of built.rows) for (const entry of row.entries) for (const name of peopleOf(entry.holder)) if (!owners.has(name)) lose(name, row.permission);
    }
    for (const request of wanted.rows ?? []) {
      for (const row of rowsOf(request.permission)) {
        for (const id of request.remove ?? []) for (const entry of row.entries.filter((candidate) => candidate.id === id)) for (const name of peopleOf(entry.holder)) lose(name, row.permission);
        if (request.inherit === false) for (const holder of row.inherited?.holders ?? []) for (const name of peopleOf(holder)) if (!owners.has(name)) lose(name, row.permission);
      }
    }
    const ordered: PreviewPerson[] = [...losing.entries()].flatMap(([username, permissions]) => {
      const found = this.person(username);
      return found === undefined ? [] : [{ user: this.ref(found), permissions: [...permissions], pages: 1 }];
    });
    const SHOWN = 5;
    return { gaining: [], losing: ordered.slice(0, SHOWN), more: Math.max(0, ordered.length - SHOWN), pages: page.defaults ? this.followers(page) : 1, orgs: null };
  }

  /** An ordinary page/role producer runs against cloned access data, then is forgotten. No live world frame is published. */
  private sharePreview(person: Person, path: string, id: string, body: SharePreviewBody): SharePreview {
    if (body.kind === 'agents') {
      const pages=this.pages, lines=this.lines, clock=this.clock, refusals=this.refusals, sent=this.sent.length;
      this.pages=structuredClone(pages);this.lines=structuredClone(lines);this.refusals=structuredClone(refusals);
      try { return this.shares.preview(person,path,id,body.share); }
      finally {this.pages=pages;this.lines=lines;this.clock=clock;this.refusals=refusals;this.sent.length=sent;}
    }
    const source=this.pageOf(path,id,false);
    if (path === 'nodes') this.world.places.node(person,id);
    else this.must(person,source);
    if (!this.mayShare(person,source)) throw this.lacking(source,['access.manage']);
    if (body.kind === 'page' && (body.change.whole !== undefined || body.change.owners !== undefined || (body.change.rows ?? []).some((row) => row.inherit !== undefined || row.locked !== undefined || row.update !== undefined || row.remove !== undefined))) throw new Refusal(400,'request.invalid','a share preview only adds holders');
    const pages=this.pages, lines=this.lines, roles=this.explicitRoles, refusals=this.refusals, providerOpenTo=this.providerOpenTo, clock=this.clock, sent=this.sent.length;
    const locations=body.kind === 'role' && path === 'nodes' ? [id,...[...this.world.trees.values()].flatMap((tree) => [...tree.nodes.values()]).filter((node) => node.id !== id && this.world.places.pathTo(node.id).some((part) => part.id === id)).map((node) => node.id)] : [id];
    const policy=this.sharingPolicy(source);
    const ruleSource=(at:string | null): NodeOrSystem | null => {
      if (at === null) return null;
      const ref=this.nodeRef(at);
      return ref === 'system' || this.world.places.access(person,at).permissions.includes(`${ref.kind}.view`) ? ref : {...ref,name:null,redacted:true};
    };
    const answer:SharePreview={allowed:false,revision:source.revision,permissions:[],holders:[],effects:{pages:0,orgs:null,gaining:[],losing:[],more:0},already:[],already_more:0,agents:[],rows:[],
      rules:[{key:'sharing.allow_holders',value:policy.value,source:ruleSource(policy.at),sharing:true},{key:'sharing.allow_external',value:this.ownersMayShareOutside,source:this.ownersMayShareOutside ? null : 'system',sharing:true}],refusal:null};
    this.pages=structuredClone(pages);this.lines=structuredClone(lines);this.explicitRoles=structuredClone(roles);this.refusals=new Map(refusals);this.providerOpenTo=new Set(providerOpenTo);
    try {
      const considered=body.kind === 'page' ? [...new Set([...(body.change.rows ?? []).map((row) => row.permission),...(body.change.name ?? []).flatMap((name) => CATALOGUE.choices.find((choice) => choice.key === name.choice)?.permissions[source.kind] ?? [])])] : CATALOGUE.permissions.filter((permission) => source.rows.has(`${permission.name}|any`)).map((permission) => permission.name);
      const inputs:HolderInput[]=body.kind === 'page' ? [...(body.change.name ?? []).flatMap((name) => name.holders),...(body.change.rows ?? []).flatMap((row) => (row.add ?? []).map((entry) => entry.holder))] : [body.give.subject.kind === 'role' ? {kind:'role',role:body.give.subject.role ?? 'viewer',at:body.give.subject.id} : {kind:body.give.subject.kind,id:body.give.subject.id}];
      const held=(who:Person,place:string): Set<string> => {const page=this.pageOf(path,place,false);return this.heldOn(who,page,this.rowsFor(page,who).rows);};
      const readers=[...this.world.people.values()].filter((reader) => !reader.suspended);
      const before=new Map(readers.map((reader) => [reader.me.id,new Map(locations.map((place) => [place,held(reader,place)]))]));
      const selected=inputs.map((input) => this.holderOf(input,source).holder);
      const beforeRows=this.build(this.pageOf(path,id,false),person).rows;
      if (body.kind === 'page') this.change(person,this.pageOf(path,id,false),body.change);
      else {
        if (path !== 'nodes') throw new Refusal(400,'request.invalid','roles are given on places');
        this.giveRole(person,id,body.give);
      }
      answer.allowed=true;answer.holders=selected;answer.permissions=considered;
      const afterRows=this.build(this.pageOf(path,id,false),person).rows;
      for (const old of beforeRows) {
        const after=afterRows.find((row) => row.permission === old.permission && row.profile === old.profile);
        const wanted=(entries:Entry[]): Entry[] => entries.filter((entry) => selected.some((holder) => JSON.stringify(holder) === JSON.stringify(entry.holder)));
        const was=wanted(old.entries),now=wanted(after?.entries ?? []);
        if (JSON.stringify(was) !== JSON.stringify(now)) answer.rows.push({defaults:false,permission:old.permission,profile:old.profile,before:was,after:now});
      }
      const namedDirect=new Set(selected.flatMap((holder) => holder.kind === 'user' ? [holder.user.id] : []));
      const root=this.rootOf(path,id);
      const known=(reader:Person): boolean => reader.me.id === person.me.id || person.me.is_system_admin || (this.isMember(person,root) && this.isMember(reader,root)) || namedDirect.has(reader.me.id);
      for (const reader of readers) {
        const gain=new Set<string>(),lose=new Set<string>();let changed=0;
        for (const place of locations) {
          const was=before.get(reader.me.id)?.get(place) ?? new Set<string>();const now=held(reader,place);
          const added=[...now].filter((permission) => !was.has(permission)),removed=[...was].filter((permission) => !now.has(permission));
          if (added.length > 0 || removed.length > 0) changed += 1;
          for (const permission of added) gain.add(permission);for (const permission of removed) lose.add(permission);
        }
        if (gain.size > 0 || lose.size > 0) {
          if (!known(reader)) answer.effects.more += 1;
          else {
            if (gain.size > 0) answer.effects.gaining.push({user:this.ref(reader),permissions:[...gain],pages:changed});
            if (lose.size > 0) answer.effects.losing.push({user:this.ref(reader),permissions:[...lose],pages:changed});
          }
        }
        if (selected.some((holder) => this.isHolder(reader,holder,id))) {
          const already=considered.filter((permission) => before.get(reader.me.id)?.get(id)?.has(permission) === true);
          if (already.length > 0) {
            if (known(reader)) answer.already.push({user:this.ref(reader),permissions:already,pages:1});else answer.already_more += 1;
          }
        }
      }
      answer.effects.pages=locations.length;
      // Match the API's bound: every unnamed or excess person is counted without giving their name.
      const shown=200;
      for (const people of [answer.effects.gaining,answer.effects.losing]) {
        people.sort((one,other) => other.pages-one.pages || one.user.id.localeCompare(other.user.id));
        answer.effects.more += Math.max(0,people.length-shown);people.splice(shown);
      }
      answer.already.sort((one,other) => one.user.id.localeCompare(other.user.id));
      answer.already_more += Math.max(0,answer.already.length-shown);answer.already.splice(shown);
      return answer;
    } catch (error) {
      if (!(error instanceof Refusal)) throw error;
      answer.refusal=error.answer.body as Problem;
      return answer;
    } finally {this.pages=pages;this.lines=lines;this.explicitRoles=roles;this.refusals=refusals;this.providerOpenTo=providerOpenTo;this.clock=clock;this.sent.length=sent;}
  }

  // ---------------------------------------------------------------- roles, groups, finding people, asking

  private roleRow(username: string, role: Role, node: string, id: string, domain: boolean, revision: number): RoleHeld {
    const found = this.person(username)!;
    return {
      id, node: this.nodeRef(node) as NodeRef, subject: { kind: 'user', id: found.me.id, user: this.ref(found) }, role, domain, outside: false,
      expires_at: null, note: '', created_by: this.ref(this.person('ada')!), created_at: '2026-10-01T09:00:00.000Z', revision,
    };
  }

  /** The roles a person may give in a place: those up to their own where they may share it, as an owner, as a manager of its page or because the place lets holders share. */
  private rolesGivable(person: Person, nodeId: string): Role[] {
    const page = this.pageOf('nodes', nodeId, false);
    const own: Role | null = this.owns(person, page) ? 'admin' : this.roleOf(person, nodeId);
    if (own === null || !this.mayShare(person, page)) return [];
    return ROLES.filter((role) => rank(role) <= rank(own));
  }

  private roleList(person: Person, id: string): { owners: Holder[]; own: RoleHeld[]; above: RoleHeld[]; can_give: Role[] } {
    this.world.places.node(person, id);
    const root = this.rootOf('nodes', id);
    const chain = this.above(id);
    const domain = (place: string): RoleHeld[] =>
      [...(this.world.places.roles.get(root)?.entries() ?? [])].filter(([, held]) => held.domain === place).map(([username, held]) => this.roleRow(username, held.role, place, held.roleId, true, held.revision));
    const explicit = (place: string): RoleHeld[] => this.explicitRoles.filter((entry) => entry.node === place).map((entry) => entry.role);
    return {
      owners: this.pageOf('nodes', id, false).owners.map((entry) => entry.holder),
      own: [...domain(id), ...explicit(id)],
      above: chain.filter((place) => place !== 'system').flatMap((place) => [...domain(place), ...explicit(place)]),
      can_give: this.rolesGivable(person, id),
    };
  }

  /** The moment the fake world is at, for what is given now: it moves a second with every change. */
  private stamp(): string {
    this.clock += 1;
    return new Date(Date.parse('2026-10-11T10:00:00.000Z') + this.clock * 1_000).toISOString();
  }

  private giveRole(person: Person, nodeId: string, body: Record<string, unknown>): RoleHeld {
    const page = this.pageOf('nodes', nodeId, false);
    this.world.places.node(person, nodeId);
    const subject = body.subject as { kind: string; id: string; role?: Role };
    const role = body.role as Role;
    const root = this.rootOf('nodes', nodeId);
    // Nobody gives a role above their own, and where the place keeps sharing to owners, only an owner or a manager of its page gives any.
    if (!this.rolesGivable(person, nodeId).includes(role)) throw this.lacking(page, ['access.manage']);
    const taken = this.roleList(person, nodeId).own.some((held) => held.subject.kind === subject.kind && held.subject.id === subject.id && held.subject.role === subject.role);
    if (taken) throw new Refusal(409, 'conflict.exists', 'that subject holds a role there already');
    const id = randomUUID();
    let row: RoleHeld;
    if (subject.kind === 'group') {
      const group = this.groups.find((entry) => entry.group.id === subject.id);
      if (group === undefined) throw new Refusal(404, 'not_found', 'no such group');
      row = { ...this.roleRow('ada', role, nodeId, id, false, 1), subject: { kind: 'group', id: group.group.id, group: { ...group.group, member_count: group.members.length } } };
    } else if (subject.kind === 'role' || subject.kind === 'members') {
      this.world.places.node(person, subject.id);
      const at = this.nodeRef(subject.id) as NodeRef;
      if (subject.kind === 'role' && subject.role === undefined) throw new Refusal(400, 'request.invalid', 'a role at a place is required');
      row = { ...this.roleRow('ada', role, nodeId, id, false, 1), subject: { kind: subject.kind, id: subject.id,
        ...(subject.kind === 'role' ? { role: subject.role, at } : { org: at }) }, outside: this.rootOf('nodes', subject.id) !== root };
    } else {
      const target = this.personById(subject.id);
      if (target === undefined) throw new Refusal(404, 'not_found', 'no such person');
      if (!this.isMember(target, root) && !this.mayShareOutside(person, page)) throw new Refusal(403, 'sharing.external_forbidden', 'naming somebody from outside takes the right to');
      row = { ...this.roleRow(target.me.username, role, nodeId, id, false, 1), outside: !this.isMember(target, root) };
    }
    row.created_by = this.ref(person);
    row.created_at = this.stamp();
    if (typeof body.expires_at === 'string') row.expires_at = body.expires_at;
    if (typeof body.note === 'string') row.note = body.note;
    this.explicitRoles.push({ role: row, node: nodeId });
    this.note(page, person, 'role.added', `${person.me.display_name} gave a role`);
    return row;
  }

  private editRole(person: Person, mine: { role: RoleHeld; node: string }, remove: boolean, body: Record<string, unknown>): Answer {
    const page = this.pageOf('nodes', mine.node, false);
    // Whoever manages the place's page changes any role; whoever gave one takes it back or changes it, for as long as they could give it.
    if (!this.mayManage(person, page)) {
      const gave = mine.role.created_by.id === person.me.id;
      const allowed = this.rolesGivable(person, mine.node);
      const next = typeof body.role === 'string' ? (body.role as Role) : mine.role.role;
      if (!gave || !allowed.includes(mine.role.role) || !allowed.includes(next)) throw this.lacking(page, ['access.manage']);
    }
    if (remove) {
      this.explicitRoles = this.explicitRoles.filter((entry) => entry !== mine);
      return { status: 204 };
    }
    if (body.expected_revision !== mine.role.revision) throw new Refusal(409, 'conflict.stale_revision', 'the role was changed meanwhile');
    if (typeof body.role === 'string') mine.role.role = body.role as Role;
    if ('expires_at' in body) mine.role.expires_at = typeof body.expires_at === 'string' ? body.expires_at : null;
    mine.role.revision += 1;
    return { status: 200, body: mine.role };
  }

  /** An invitation to one place, which a person who has no account is sent: the fake system has no way to send mail, so the link is handed back. */
  private inviteToPlace(person: Person, body: Record<string, unknown>): unknown {
    const nodeId = String(body.node_id);
    this.world.places.need(person, nodeId, 'access.manage');
    const token = `inv_${randomUUID()}`;
    const email = typeof body.email === 'string' && body.email !== '' ? body.email : null;
    return {
      invitation: {
        id: randomUUID(),
        kind: 'node',
        org: null,
        node: this.nodeRef(nodeId),
        chat: null,
        role: null,
        permissions: (body.permissions as string[] | undefined) ?? [],
        form: email === null ? 'link' : 'email',
        email,
        max_uses: 1,
        uses: 0,
        note: '',
        expires_at: new Date(Date.now() + 168 * 3_600_000).toISOString(),
        revoked: false,
        created_by: this.ref(person),
        created_at: new Date().toISOString(),
      },
      token,
      url: `http://127.0.0.1:41922/invite/${token}`,
      sent: false,
    };
  }

  // ---------------------------------------------------------------- the requests only the double has

  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/refuse':
        this.refusals.set(`${String(body.username)}:${String(body.node)}`, { permissions: (body.permissions as string[] | undefined) ?? ['node.view'] });
        return { status: 204 };
      case '/serve':
        if (typeof body.previews === 'boolean') this.previewsServed = body.previews;
        if (typeof body.requests === 'boolean') this.requestsServed = body.requests;
        return { status: 204 };
      case '/no-outsiders':
        if (body.off === true) this.noOutsiders.add(String(body.username));
        else this.noOutsiders.delete(String(body.username));
        return { status: 204 };
      case '/owners-share-outside':
        this.ownersMayShareOutside = body.allowed === true;
        return { status: 204 };
      case '/requests/limit':
        return this.requests.control(path.slice('/requests'.length), body);
      case '/shares/reach':
      case '/shares/tools-off':
      case '/shares/snapshot':
      case '/shares/list':
        return this.shares.control(path.slice('/shares'.length), body);
      case '/holders-share':
        // The policy that says whether somebody who holds a thing may share it, set at a place and holding for everything inside.
        this.allowHolders.set(String(body.place), body.allowed === true);
        return { status: 204 };
      case '/sent':
        return { status: 200, body: this.sent };
      case '/connected':
        if (body.on === true) this.connected.add(String(body.thread));
        else this.connected.delete(String(body.thread));
        return { status: 204 };
      case '/touch': {
        // Somebody else changes a page, so that the page a test's person has open is behind.
        const as = this.person(String(body.as));
        if (as === undefined) return { status: 404, body: { detail: 'no such person' } };
        const page = this.pageOf(typeof body.path === 'string' ? body.path : 'nodes', String(body.id), body.defaults === true);
        this.apply(as, page, body.change as PageChange);
        return { status: 200, body: { revision: page.revision } };
      }
      case '/lock': {
        // A lock a place set on one of its defaults rows, without going through a person.
        const page = this.pageOf('nodes', String(body.place), true);
        const row = page.rows.get(`${String(body.permission)}|${typeof body.profile === 'string' ? body.profile : 'any'}`);
        if (row === undefined) return { status: 404, body: { detail: 'no such row' } };
        row.locked = true;
        page.revision += 1;
        return { status: 204 };
      }
      default:
        return null;
    }
  }
}
