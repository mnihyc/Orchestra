/** The state of the fake API and the rules it answers by: who may sign in with what, what a refusal looks like, and what the tree holds.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: the same
 * codes, the same members beside them, the same answer whatever the name where the real one tells
 * nothing. It keeps everything in memory and forgets it when it is reset.
 */
import { createHash, randomUUID } from 'node:crypto';
import type { IncomingMessage } from 'node:http';

import type {
  AuthResult,
  AdminMode,
  Chat,
  ChatRef,
  InvitationAcceptReply,
  InvitationPreview,
  Me,
  Meta,
  Org,
  OrgTreeGetReply,
  Problem,
  TreeNode,
} from '../../src/generated/api-types.ts';
import { Access } from './access.ts';
import { AdminWorld } from './admin.ts';
import { AdminOpenings } from './admin-openings.ts';
import { BrowsersWorld } from './browsers.ts';
import { DisplaysWorld } from './displays.ts';
import { Change } from './change.ts';
import { Chats } from './chats.ts';
import { Documents } from './documents.ts';
import { previewReference } from './references.ts';
import { PeopleWorld } from './people.ts';
import { Craft } from './craft.ts';
import { Day100World } from './day100.ts';
import { ExportWorld } from './export.ts';
import * as fixtures from './fixtures.ts';
import { FirstStepsWorld } from './first-steps.ts';
import { NotificationWorld } from './notifications.ts';
import { Places } from './places.ts';
import { Profiles } from './profile.ts';
import { AdoptionWorld } from './adoption.ts';
import { PortsWorld } from './ports.ts';
import { Resources } from './resources.ts';
import { TemporaryWorld } from './temporary.ts';
import { RequestFeeds } from './request-feeds.ts';
import { Usage } from './usage.ts';
import { AwayWorld } from './away.ts';
import { SearchWorld } from './search.ts';
import { EverythingWorld } from './everything.ts';
import { AnnouncementsWorld } from './announcements.ts';
import { Settings } from './settings.ts';
import { Starts } from './starts.ts';
import { ThreadWorld } from './threads.ts';
import { TraceWorld } from './trace.ts';
import { UserFeed } from './user-feed.ts';
import { WorkspaceWorld } from './workspace.ts';

type Proof = 'password' | 'email_code' | 'totp' | 'recovery_code';
type ProofBucket = Exclude<Proof, 'recovery_code'>;

/** How many wrong tries of one proof close it for a name. */
const WRONG_TRIES = 8;
/** For how long, in seconds. */
const LOCKED_FOR_SECS = 3 * 60 * 60;
/** How long an access token is good. */
const ACCESS_LIFE_MS = 15 * 60_000;
/** The fewest characters of a password. */
const PASSWORD_MIN = 10;
const USERNAME = /^[a-z0-9][a-z0-9_-]{2,31}$/;

export interface Person {
  me: Me;
  password: string;
  /** The authenticator's code that is right, for somebody who has one. */
  totp: string | null;
  recoveryCodes: string[];
  recoveryCreatedAt: string | null;
  /** Whether a mailed code alone gets them in. */
  codeSignsIn: boolean;
  /** The code that was mailed last and is still good. */
  mailed: string | null;
  suspended: boolean;
  wrong: Record<ProofBucket, number>;
  locked: Partial<Record<ProofBucket, number>>;
}

interface Invitation {
  preview: InvitationPreview;
  /** The address a personal link is for. */
  email: string | null;
  spent: boolean;
  orgId: string | null;
  nodeId?: string;
  chatId?: string;
}

export interface Answer {
  status: number;
  body?: unknown;
  /** Plain text, for the few requests that answer in text and not in JSON. */
  text?: string;
  /** A file, for a request that answers with one: bytes with the type and the name that `headers` give them. */
  bytes?: Uint8Array;
  headers?: Record<string, string>;
  /** Cookies to set (a value) or to clear (`null`). */
  cookies?: Record<string, string | null>;
}

export interface Subscriber {
  send: (frame: unknown) => void;
  close: (code: number) => void;
}

/**
 * What a part of the fake world offers to the one server (see `World.parts`). A part leaves out what it does not use, and answers
 * only what is its own: one thing has one answer, whichever screen asks.
 */
export interface Part {
  /** The requests of the API whose body it reads itself, because it is not JSON (a file's bytes) or is wanted in ranges; asked before `handle`, with the request as it came. `null` for one that is not its own. */
  raw?: (person: Person, request: IncomingMessage, method: string, path: string, query: URLSearchParams) => Promise<Answer | null>;
  /** The requests of the API that it answers (at once, or later when it is made to wait); `null` for one that is not its own. */
  handle?: (person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>, actKey?: string, requestId?: string) => Answer | null | Promise<Answer | null>;
  /** The requests only the double has, `/__double/<name>/...`, by the path after the name; `null` for one that is not its own. */
  control?: (path: string, body: Record<string, unknown>) => Answer | null;
  /** A frame of the live connection (`sub`, `unsub`, `visible`, `typing`) about a feed that it serves; whether it was its own. */
  frame?: (subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>) => boolean;
  /** A live connection ended. */
  gone?: (subscriber: Subscriber) => void;
}

export class Refusal extends Error {
  readonly answer: Answer;

  constructor(status: number, code: Problem['code'], detail: string, extra: Record<string, unknown> = {}, headers: Record<string, string> = {}) {
    super(`${code}: ${detail}`);
    this.answer = {
      status,
      headers,
      body: { type: `orch:error:${code}`, code, title: detail, status, detail, request_id: '', ...extra },
    };
  }
}

const refuse = (
  status: number,
  code: Problem['code'],
  detail: string,
  extra?: Record<string, unknown>,
  headers?: Record<string, string>,
): never => {
  throw new Refusal(status, code, detail, extra, headers);
};

export interface Scenario {
  /** `fresh`: a system with no user. `team`: a team at work. */
  scenario?: 'fresh' | 'team';
  meta?: Partial<Meta>;
  /** Milliseconds every answer is held back, to look at what a slow answer shows. */
  delayMs?: number;
  /** Whether new registrations must confirm their address first. */
  confirmAddresses?: boolean;
  /** How long an access token is good, to see a renewal without waiting a quarter of an hour. */
  accessLifeMs?: number;
}

export class World {
  /** Permanent event-time trails and retained lifecycle facts, shared by every entry point. */
  trace: TraceWorld = new TraceWorld(this);
  openings: AdminOpenings = new AdminOpenings(this);
  meta: Meta = fixtures.meta();
  people = new Map<string, Person>();
  profiles: Profiles = new Profiles(this);
  peopleWork: PeopleWorld = new PeopleWorld(this);
  sessions = new Map<string, string>();
  tokens = new Map<string, { username: string; endsAt: number; session?: string }>();
  private sessionModes = new Map<string, { id: string; asAdmin: boolean }>();
  private browserPrincipals = new WeakMap<Person, string>();
  /** The shared server drops only the changed session's former live projections. */
  sessionAuthorityChanged: ((id: string, ended: boolean) => void) | null = null;
  invitations = new Map<string, Invitation>();
  resets = new Map<string, string>();
  confirmations = new Map<string, string>();
  orgs: Org[] = [];
  places: Places = new Places(this);
  /** What threads run on: machines, providers with their models, agent profiles, environments, hosts and templates. One set for every screen that speaks of them. */
  resources: Resources = new Resources(this);
  /** Temporary export snapshots have no thread and remain downloadable after removal. */
  temporary: TemporaryWorld = new TemporaryWorld(this);
  /** One ledger shared by figures, caps, rates and thread limit prompts. */
  usage: Usage = new Usage(this);
  /** Current delivery outcomes, grouped reads and device registrations extend the same notifications. */
  away: AwayWorld = new AwayWorld(this);
  /** Current listeners and deliberate openings, shared by every environment screen. */
  ports: PortsWorld = new PortsWorld(this);
  /** The sessions the harnesses of the machines hold that were begun outside Orch, and adopting one. */
  adoption: AdoptionWorld = new AdoptionWorld(this);
  settings: Settings = new Settings(this);
  /** Fixtures and personal views which use the same settings and places as every other screen. */
  day100: Day100World = new Day100World(this);
  threads: ThreadWorld = new ThreadWorld(this);
  /** What starting a thread is asked from: the defaults of each place, and what a thread there could be bound to before it exists. */
  starts: Starts = new Starts(this);
  /** The permission page of every thing, the defaults of every place, roles held in places, and asking for access. */
  access: Access = new Access(this);
  /** What is asked to be changed (a fork, a switch, an update, a move, a recycle), the jobs that carry it out, and the side threads of a thread. */
  change: Change = new Change(this);
  /** Chats among people, and what is forwarded to them. */
  chats: Chats = new Chats(this);
  /** Taking things away: one thing as one file, the archive of a place and the job that writes it. */
  exports: ExportWorld = new ExportWorld(this);
  /** The feed of each person: what happens to them wherever they look. Every part that has something to tell a person tells it here. */
  users: UserFeed = new UserFeed();
  /** Authorized native snapshots and job completions use the same live protocol as the application. */
  requestFeeds: RequestFeeds = new RequestFeeds(this);
  /** The names the finder lists and the search of what was written: read from the other worlds, kept nowhere else. */
  search: SearchWorld = new SearchWorld(this);
  /** The complete visible hierarchy reuses the same places, documents and resources. */
  everything: EverythingWorld = new EverythingWorld(this);
  announcements: AnnouncementsWorld = new AnnouncementsWorld(this);
  /** The pages of blocks written beside a node, their comments, versions and the signs of who is writing. */
  documents: Documents = new Documents(this);
  craft: Craft = new Craft(this);
  /** What an environment holds that a screen looks into: its files, Git, processes, resources and shells, and the outline and parameters of a thread. */
  workspace: WorkspaceWorld = new WorkspaceWorld(this);
  /** A person's Inbox and its counters, the approvals that wait for them (of the three kinds, listed here and decided where they are kept), and the ways they are told. */
  notifications: NotificationWorld = new NotificationWorld(this);
  /** The chosen org's observed setup facts, read from these same worlds. */
  firstSteps: FirstStepsWorld = new FirstStepsWorld(this);
  /** The system's own screens: its state and measures, root orgs, sign-in, hosts, runtimes, capabilities, harnesses, storage and authority. */
  admin: AdminWorld = new AdminWorld(this);
  /** The browsers of each environment, their profiles, and the sockets that show them and take what a person does. */
  browsers: BrowsersWorld = new BrowsersWorld(this);
  /** The displays of each environment, and the sockets that carry them as RFB. */
  displays: DisplaysWorld = new DisplaysWorld(this);
  trees = new Map<string, { seq: number; nodes: Map<string, TreeNode> }>();
  subscribers = new Map<string, Map<Subscriber, number>>();
  delayMs = 0;
  confirmAddresses = false;
  accessLifeMs = ACCESS_LIFE_MS;
  /** What was "mailed", newest last, for a test to read. */
  outbox: { to: string; kind: 'code' | 'reset' | 'confirm'; secret: string }[] = [];

  /**
   * Every part of the world, in the order in which the server offers it a request, a control or a frame of the live connection. Each
   * answers what is its own and leaves the rest, so that a request has one answer whichever screen asks: a part that is added is one
   * line here. `name` is what its controls are asked by (`/__double/<name>/...`). The accounts, the sessions and the tree of an
   * org are the world's own.
   */
  get parts(): { name: string; part: Part }[] {
    return [
      { name: 'references', part: {handle:(person,method,path,query)=>previewReference(this,person,method,path,query)} },
      { name: 'request-feeds', part: this.requestFeeds },
      // First: a person's own project views and starred views, a topic's status against its revision, and a tag made where it is picked.
      { name: 'day100', part: this.day100 },
      { name: 'first-steps', part: this.firstSteps },
      { name: 'everything', part: this.everything },
      { name: 'announcements', part: this.announcements },
      // First: it sees every request that changes something before whoever answers it, and the defaults of a topic are read and written here before the threads say anything of them.
      { name: 'start', part: this.starts },
      { name: 'trace', part: this.trace },
      // After the trace, whose are the people and the trail of the system; before the places, which know no root org it made up.
      { name: 'admin', part: this.admin },
      { name: 'temporary', part: this.temporary },
      // Before the resources and the threads: forks, switches, updates, moves and recycles, their jobs, and side threads are its own.
      { name: 'change', part: this.change },
      { name: 'ports', part: this.ports },
      { name: 'adoption', part: this.adoption },
      // Before the resources: the browsers and displays of an environment are their own.
      { name: 'browsers', part: this.browsers },
      { name: 'displays', part: this.displays },
      { name: 'resources', part: this.resources },
      { name: 'usage', part: this.usage },
      // Before the threads, the access part and the settings: one list of what waits for a person is theirs, and the person's own ways of being told.
      { name: 'away', part: this.away },
      { name: 'notifications', part: this.notifications },
      // Workspace feeds use environment identities; reject unsupported thread aliases before the thread pattern consumes them.
      { name: 'workspace', part: this.workspace },
      { name: 'people', part: this.peopleWork },
      { name: 'thread', part: this.threads },
      { name: 'access', part: this.access },
      { name: 'chats', part: this.chats },
      { name: 'export', part: this.exports },
      { name: 'user', part: this.users },
      // Before the places: it adds to what they answer of the members of an org, and answers the settings of a node the places only stubbed.
      { name: 'settings', part: this.settings },
      { name: 'profile', part: this.profiles },
      { name: 'places', part: this.places },
      { name: 'documents', part: this.documents },
      { name: 'craft', part: this.craft },
      { name: 'search', part: this.search },
    ];
  }

  constructor(scenario: Scenario = {}) {
    this.reset(scenario);
  }

  reset(scenario: Scenario = {}): void {
    this.requestFeeds.reset();
    for (const sockets of this.subscribers.values()) for (const socket of sockets.keys()) socket.close(1012);
    this.people.clear();
    this.profiles.reset();
    this.sessions.clear();
    this.tokens.clear();
    this.sessionModes.clear();
    this.browserPrincipals = new WeakMap();
    this.invitations.clear();
    this.resets.clear();
    this.confirmations.clear();
    this.subscribers.clear();
    this.strangers.clear();
    this.trees.clear();
    this.orgs = [];
    this.places = new Places(this);
    this.resources.reset();
    this.temporary = new TemporaryWorld(this);
    this.usage.reset();
    this.change.reset();
    this.ports.reset();
    this.adoption.reset();
    this.settings = new Settings(this);
    this.day100 = new Day100World(this);
    this.starts.reset();
    this.threads = new ThreadWorld(this);
    this.access = new Access(this);
    this.chats = new Chats(this);
    this.exports = new ExportWorld(this);
    this.users = new UserFeed();
    this.search.reset();
    this.everything.reset();
    this.announcements.reset();
    this.documents = new Documents(this);
    this.peopleWork = new PeopleWorld(this);
    this.workspace = new WorkspaceWorld(this);
    this.notifications.reset();
    this.away.reset();
    this.firstSteps = new FirstStepsWorld(this);
    this.users.inbox = this.notifications;
    this.outbox = [];
    this.delayMs = scenario.delayMs ?? 0;
    this.confirmAddresses = scenario.confirmAddresses ?? false;
    this.accessLifeMs = scenario.accessLifeMs ?? ACCESS_LIFE_MS;
    const fresh = scenario.scenario === 'fresh';
    this.meta = fixtures.meta({ needs_setup: fresh, ...scenario.meta });
    if (!fresh) this.seedTeam();
    // Somebody of no org of the tree, for what is shared outside, once the world has its people.
    this.access.seed();
    this.trace = new TraceWorld(this);
    if (!fresh) this.trace.seed();
    // The system's own screens read the people the world was just given.
    this.admin = new AdminWorld(this);
    // Browsers and displays are opened by the people the world was just given.
    this.browsers.reset();
    this.displays.reset();
  }

  private seedTeam(): void {
    const ada = this.addPerson(fixtures.me(1, 'ada', 'Ada Lovelace', { is_system_admin: true }), 'correct horse battery');
    this.addPerson(fixtures.me(2, 'grace', 'Grace Hopper', { totp_enabled: true }), 'tabulating machine', { totp: '246810' });
    this.addPerson(
      fixtures.me(3, 'linus', 'Linus Pauling', { email_code: ['second_step'], email_code_second_step: true }),
      'double helix model',
    );
    this.addPerson(fixtures.me(4, 'margaret', 'Margaret Hamilton', { email_code: ['sign_in'] }), 'apollo guidance computer', {
      codeSignsIn: true,
    });
    this.addPerson(fixtures.me(5, 'bolted', 'Bo Lted'), 'locked out of here', {
      locked: { password: Date.now() + LOCKED_FOR_SECS * 1_000 },
    });
    this.addPerson(fixtures.me(6, 'unconfirmed', 'Una Confirmed', { email_verified: false }), 'waiting for a link');
    this.addPerson(fixtures.me(7, '王小明', '王小明'), 'correct horse battery');
    this.addPerson(fixtures.me(8, 'vera', 'Vera Rubin'), 'galaxy rotation curves');
    this.addPerson(fixtures.me(9, 'dora', 'Dora Developer'), 'compilers and goto');

    const owner = fixtures.userRef(ada.me);
    const admin = { administers: true, owns: true, role: 'admin' as const };
    this.orgs.push(
      fixtures.org(fixtures.ACME.org, 'acme', 'Acme', owner, {
        member_count: 7,
        org_count: 1,
        project_count: 2,
        access: { ...fixtures.memberAccess, ...admin, permissions: ['node.view', 'node.manage'] },
      }),
    );
    this.trees.set(fixtures.ACME.org, { seq: 23, nodes: new Map(fixtures.acmeTree().map((node) => [node.id, node])) });
    this.places.seedTeam();
    this.documents.seedTeam();
    this.settings.seedTeam();
    this.invitations.set('good-invitation', {
      preview: fixtures.invitation(owner),
      email: null,
      spent: false,
      orgId: fixtures.ACME.org,
    });
    this.invitations.set('personal-invitation', {
      preview: fixtures.invitation(owner, { email_bound: true, role: 'viewer', org: { name: 'Research', slug: 'research' } }),
      email: 'dana@example.org',
      spent: false,
      orgId: fixtures.ACME.org,
    });
    this.invitations.set('topic-invitation', {
      preview: fixtures.invitation(owner, { kind: 'node', org:null, role: null, permissions:['topic.view'], what: { kind: 'topic', name: 'Fix login' } }),
      email: null,
      spent: false,
      orgId: null,
      nodeId: fixtures.ACME.fixLogin,
    });
    this.invitations.set('spent-invitation', { preview: fixtures.invitation(owner), email: null, spent: true, orgId: null });
  }

  addPerson(me: Me, password: string, more: Partial<Person> = {}): Person {
    const person: Person = {
      me,
      password,
      totp: null,
      recoveryCodes: [],
      recoveryCreatedAt: null,
      codeSignsIn: false,
      mailed: null,
      suspended: false,
      wrong: { password: 0, email_code: 0, totp: 0 },
      locked: {},
      ...more,
    };
    this.people.set(me.username, person);
    // An account without a personal home is a complete identity by itself.
    if (me.personal_org_id === null) return person;
    const own = fixtures.org(me.personal_org_id, me.username, me.display_name, fixtures.userRef(me), {
      org_kind: 'personal',
      closed: true,
      flat: true,
      project_count: 1,
      access: {
        admin_only: [],
        participating: true,
        administers: true,
        owns: true,
        permissions: ['node.view', 'node.manage'],
        role: 'admin',
        role_from: null,
        subscription: null,
        effective_subscription: 'off',
      },
    });
    this.orgs.push(own);
    this.trees.set(own.id, {
      seq: 2,
      nodes: new Map(fixtures.personalTree(own.id, me.display_name, me.username).map((node) => [node.id, node])),
    });
    for (const node of this.trees.get(own.id)?.nodes.values() ?? []) this.places.metaOf(node.id).owner = me.username;
    this.places.metaOf(own.id).slug = me.username;
    return person;
  }

  // ---------------------------------------------------------------- who is asking

  private find(login: string): Person | undefined {
    const name = login.trim().toLowerCase();
    for (const person of this.people.values()) {
      if (person.me.username.toLowerCase() === name || person.me.email?.toLowerCase() === name) return person;
    }
    return undefined;
  }

  private signedIn(username: string): Answer {
    const person = this.people.get(username);
    if (person === undefined) return refuse(401, 'auth.required', 'no such session');
    const session = `ors_${randomUUID()}`;
    this.sessions.set(session, username);
    this.sessionModes.set(session, { id: randomUUID(), asAdmin: false });
    return { status: 200, body: this.grant(person, session), cookies: { orch_session: session, orch_device: `odv_${randomUUID()}` } };
  }

  private grant(person: Person, session: string): AuthResult {
    const token = `oat_${randomUUID()}`;
    const endsAt = Date.now() + this.accessLifeMs;
    this.tokens.set(token, { username: person.me.username, endsAt, session });
    return { access_token: token, access_expires_at: new Date(endsAt).toISOString(), user: person.me };
  }

  /** The person a bearer token names; refuses a request that carries none, or one that is no longer good. */
  caller(authorization: string | undefined): Person {
    const token = /^Bearer (.+)$/.exec(authorization ?? '')?.[1];
    if (token === undefined) return refuse(401, 'auth.required', 'no credentials');
    const held = this.tokens.get(token);
    const person = held === undefined ? undefined : this.people.get(held.username);
    if (held === undefined || person === undefined || held.endsAt <= Date.now()
      || (held.session !== undefined && this.sessions.get(held.session) !== held.username)) {
      return refuse(401, 'auth.invalid_token', 'the access token is expired or unknown');
    }
    if (person.suspended) return refuse(401, 'auth.suspended', 'the user is suspended');
    return person;
  }

  /** Availability is a current role fact, never a grant made by the switch. */
  adminModeAvailable(person: Person): boolean {
    const current = this.people.get(person.me.username);
    if (current === undefined || current.suspended) return false;
    if (current.me.is_system_admin) return true;
    return this.orgs.some((org) => org.org_kind === 'team' && [org.id, ...(this.trees.get(org.id)?.nodes.keys() ?? [])]
      .some((node) => this.places.roleAt(current, node) === 'admin'));
  }

  private browserSession(authorization: string | undefined): { person: Person; key: string; mode: { id: string; asAdmin: boolean } } {
    const person = this.caller(authorization);
    const token = /^Bearer (.+)$/.exec(authorization ?? '')?.[1];
    const key = token === undefined ? undefined : this.tokens.get(token)?.session;
    if (key === undefined) return refuse(403, 'auth.session_required', 'administrator mode belongs to a browser session');
    const mode = this.sessionModes.get(key);
    if (mode === undefined) return refuse(401, 'auth.invalid_token', 'this session has ended');
    return { person, key, mode };
  }

  adminMode(authorization: string | undefined): AdminMode {
    const { person, mode } = this.browserSession(authorization);
    return { as_admin: mode.asAdmin, available: this.adminModeAvailable(person) };
  }

  /** Trusted opening provenance, freshly bound to this bearer/session. */
  openingIdentity(authorization: string | undefined): { person: Person; session: string; asAdmin: boolean } {
    const { mode } = this.browserSession(authorization);
    return { person: this.browserPerson(authorization), session: mode.id, asAdmin: mode.asAdmin };
  }

  setAdminMode(authorization: string | undefined, body: Record<string, unknown>): Answer {
    const { person, mode } = this.browserSession(authorization);
    if (Object.keys(body).length !== 1 || typeof body.as_admin !== 'boolean') {
      return refuse(400, 'request.invalid', 'the request is not valid', { errors: [{ path: 'as_admin', message: 'must be the only member and a boolean' }] });
    }
    const available = this.adminModeAvailable(person);
    if (body.as_admin && !available) return refuse(403, 'authz.denied', 'you administer no place');
    if (mode.asAdmin !== body.as_admin) {
      mode.asAdmin = body.as_admin;
      this.users.tell(person.me.username, { what: 'admin_mode', session_id: mode.id, as_admin: body.as_admin });
      this.sessionAuthorityChanged?.(mode.id, false);
    }
    return { status: 200, body: { as_admin: mode.asAdmin, available } satisfies AdminMode };
  }

  /** Existing parts resolve system reach through this request projection. The
   * identity record and its ordinary roles are never changed or copied to a peer. */
  browserPerson(authorization: string | undefined): Person {
    const person = this.caller(authorization);
    const token = /^Bearer (.+)$/.exec(authorization ?? '')?.[1];
    const key = token === undefined ? undefined : this.tokens.get(token)?.session;
    const actor = new Proxy(person, {
      get: (target, property, receiver): unknown => property !== 'me' ? Reflect.get(target, property, receiver) : new Proxy(target.me, {
        get: (me, field, owner): unknown => field !== 'is_system_admin' ? Reflect.get(me, field, owner)
          : me.is_system_admin && !target.suspended && key !== undefined
            && this.sessions.get(key) === target.me.username && this.sessionModes.get(key)?.asAdmin === true,
      }),
      set: (target, property, value, receiver) => {
        // Personal mutators spread their request projection. Its masked system
        // reach is not an identity edit; native admin changes use the raw target.
        if (property === 'me') { target.me = { ...(value as Me), is_system_admin: target.me.is_system_admin }; return true; }
        return Reflect.set(target, property, value, receiver);
      },
    });
    if (key !== undefined) this.browserPrincipals.set(actor, key);
    return actor;
  }

  browserSessionRefs(person: Person): { id: string; current: boolean }[] | null {
    const current = this.browserPrincipals.get(person);
    if (current === undefined) return null;
    return [...this.sessionModes].flatMap(([key, mode]) => this.sessions.get(key) === person.me.username
      ? [{ id: mode.id, current: key === current }] : []);
  }

  revokeBrowserSessions(person: Person, id?: string): void {
    const current = this.browserPrincipals.get(person);
    if (current === undefined) return;
    for (const [key, mode] of this.sessionModes) {
      if (key !== current && this.sessions.get(key) === person.me.username && (id === undefined || mode.id === id)) this.forgetSession(key);
    }
  }

  private forgetSession(key: string): void {
    const id = this.sessionModes.get(key)?.id;
    this.sessions.delete(key);
    this.sessionModes.delete(key);
    for (const [token, held] of this.tokens) if (held.session === key) this.tokens.delete(token);
    if (id !== undefined) this.sessionAuthorityChanged?.(id, true);
  }

  /** What the system says of itself, with whether the caller may make a team org now: nobody signed in may not. */
  metaFor(authorization: string | undefined): Meta {
    const token = /^Bearer (.+)$/.exec(authorization ?? '')?.[1];
    const person = token === undefined ? null : this.tokenUser(token);
    return { ...this.meta, may_create_org: person !== null && this.admin.mayMakeOrg(person) };
  }

  /** The user a token names, for the socket, or `null`. */
  tokenUser(token: string): Person | null {
    try { return this.caller(`Bearer ${token}`); }
    catch (error) { if (error instanceof Refusal) return null; throw error; }
  }

  // ---------------------------------------------------------------- setup and sign-in

  setup(body: Record<string, unknown>): Answer {
    if (!this.meta.needs_setup) return refuse(409, 'conflict.exists', 'the system is set up already');
    const { username, display_name, password, email } = this.identity(body, false);
    const person = this.addPerson(
      fixtures.me(1, username, display_name, { is_system_admin: true, email, email_verified: false }),
      password,
    );
    this.meta = { ...this.meta, needs_setup: false };
    return this.signedIn(person.me.username);
  }

  /** Checks what a registration or a setup gives, as the real API does, naming each fault by its member. */
  private identity(body: Record<string, unknown>, emailRequired: boolean, hideEmailConflict = false) {
    const text = (member: string): string => {
      const value = body[member];
      return typeof value === 'string' ? value : '';
    };
    const username = text('username');
    const display_name = text('display_name').trim();
    const password = text('password');
    const email = text('email').trim() === '' ? null : text('email').trim();
    const errors: { path: string; message: string }[] = [];
    if (!USERNAME.test(username)) errors.push({ path: 'username', message: 'must be 3 to 32 of a-z, 0-9, - and _' });
    if (display_name === '') errors.push({ path: 'display_name', message: 'must not be empty' });
    if (Array.from(password).length < PASSWORD_MIN) errors.push({ path: 'password', message: `must be at least ${PASSWORD_MIN} characters` });
    else if (password.toLowerCase() === username || (email !== null && password.toLowerCase() === email.toLowerCase())) {
      errors.push({ path: 'password', message: 'must differ from the username and the address' });
    }
    if (email === null && emailRequired) errors.push({ path: 'email', message: 'is required here' });
    if (email !== null && !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) errors.push({ path: 'email', message: 'is not an address' });
    if (errors.length > 0) refuse(400, 'request.invalid', 'the request is not valid', { errors });
    if (this.publicNameTaken(username)) refuse(409, 'conflict.exists', 'that public name is taken', {errors:[{path:'username',message:'is taken by a username or a root org'}]});
    if (!hideEmailConflict && email !== null && this.find(email) !== undefined) refuse(409,'conflict.exists','the account change could not be completed');
    return { username, display_name, password, email };
  }

  private publicNameTaken(name: string): boolean {
    return this.people.has(name) || this.orgs.some((org) => org.parent_id === null && org.slug === name);
  }

  nameAvailability(name: string): Answer {
    if (!USERNAME.test(name)) return refuse(400,'request.invalid','must be a public name, not an address',{errors:[{path:'name',message:'must be a public name, not an address'}]});
    return {status:200,body:{available:!this.publicNameTaken(name)}};
  }

  private tried(person: Person | undefined, name: string, way: Proof, right: boolean): void {
    const counts = person ?? this.stranger(name);
    const bucket = way === 'recovery_code' ? 'totp' : way;
    const until = counts.locked[bucket];
    if (until !== undefined && until > Date.now()) {
      refuse(423, 'auth.locked', `too many wrong tries of the ${way} at this name`, { way }, {
        'Retry-After': String(Math.ceil((until - Date.now()) / 1_000)),
      });
    }
    if (right) {
      counts.wrong[bucket] = 0;
      return;
    }
    counts.wrong[bucket] += 1;
    if (counts.wrong[bucket] >= WRONG_TRIES) counts.locked[bucket] = Date.now() + LOCKED_FOR_SECS * 1_000;
  }

  private strangers = new Map<string, Pick<Person, 'wrong' | 'locked'>>();

  /** A name nobody has is counted and locked like one that somebody has. */
  private stranger(name: string): Pick<Person, 'wrong' | 'locked'> {
    const key = name.trim().toLowerCase();
    let counts = this.strangers.get(key);
    if (counts === undefined) {
      counts = { wrong: { password: 0, email_code: 0, totp: 0 }, locked: {} };
      this.strangers.set(key, counts);
    }
    return counts;
  }

  login(body: Record<string, unknown>): Answer {
    const login = typeof body.login === 'string' ? body.login : '';
    const given = (way: Proof): string | null => {
      const value = body[way];
      return typeof value === 'string' && value !== '' ? value : null;
    };
    const password = given('password');
    const code = given('email_code');
    const totp = given('totp');
    const recovery = given('recovery_code');
    if (recovery !== null && (totp !== null || password !== null && code !== null)) return refuse(400, 'request.invalid', 'give one second-step code');
    if (login.trim() === '' || (password === null && code === null)) {
      return refuse(400, 'request.invalid', 'a name and a proof are needed', {
        errors: [{ path: login.trim() === '' ? 'login' : 'password', message: 'must not be empty' }],
      });
    }
    const person = this.find(login);
    const normal = (typed: string) => typed.replace(/[\s-]/g, '').toUpperCase();
    const rightPassword = password !== null && person?.password === password;
    const mailed = person?.mailed ?? null;
    const rightCode = code !== null && mailed !== null && normal(mailed) === normal(code);

    // The first step: a password, or a mailed code where one may get this person in.
    let first: Proof;
    if (code !== null && (password === null || person?.codeSignsIn === true)) {
      first = 'email_code';
      this.tried(person, login, 'email_code', rightCode && person?.codeSignsIn === true);
      if (!rightCode || person?.codeSignsIn !== true) return refuse(401, 'auth.invalid_credentials', 'wrong name, password or code');
    } else {
      first = 'password';
      this.tried(person, login, 'password', rightPassword);
      if (!rightPassword) return refuse(401, 'auth.invalid_credentials', 'wrong name, password or code');
    }
    // What depends on the person is said only after a first step that was right.
    if (person.suspended) return refuse(401, 'auth.suspended', 'the user is suspended');
    if (!this.meta.local_login && !person.me.is_system_admin) {
      return refuse(403, 'auth.local_login_disabled', 'signing in by a password is off');
    }
    if (this.meta.mail && person.me.email !== null && !person.me.email_verified && !person.me.is_system_admin) {
      return refuse(403, 'auth.email_unverified', 'the address is not confirmed');
    }

    // The second step: another proof than the first, for somebody who has one.
    const methods: Proof[] = [];
    if (person.totp !== null) methods.push('totp');
    if (person.totp !== null && person.recoveryCodes.length > 0) methods.push('recovery_code');
    if (person.me.email_code_second_step && first !== 'email_code') methods.push('email_code');
    if (person.me.email_code_second_step && first === 'email_code' && person.totp === null) methods.push('password');
    if (methods.length > 0) {
      let second: Proof | null = null;
      if (totp !== null && methods.includes('totp')) second = 'totp';
      else if (recovery !== null && person.totp !== null) second = 'recovery_code';
      else if (code !== null && first !== 'email_code' && methods.includes('email_code')) second = 'email_code';
      else if (password !== null && first !== 'password' && methods.includes('password')) second = 'password';
      if (second === null) return refuse(401, 'auth.second_step_required', 'a second step is needed', { methods });
      const right = second === 'recovery_code' ? person.recoveryCodes.some((saved) => normal(saved) === normal(recovery ?? '')) : second === 'totp' ? totp === person.totp : second === 'email_code' ? rightCode : rightPassword;
      this.tried(person, login, second, right);
      if (!right) return refuse(401, 'auth.second_step_invalid', 'the second step was wrong');
      if (second === 'recovery_code') person.recoveryCodes = person.recoveryCodes.filter((saved) => normal(saved) !== normal(recovery ?? ''));
    }
    if (code !== null) person.mailed = null;
    return this.signedIn(person.me.username);
  }

  /** Always the same answer. A code is "mailed" where the real API would mail one. */
  emailCode(body: Record<string, unknown>): Answer {
    const login = typeof body.login === 'string' ? body.login : '';
    const password = typeof body.password === 'string' ? body.password : null;
    const person = this.find(login);
    if (person?.me.email_verified === true && person.me.email !== null) {
      const asSecond = password !== null && person.password === password && person.me.email_code_second_step;
      if (person.codeSignsIn || asSecond) {
        person.mailed = `${randomCode(4)}-${randomCode(4)}`;
        this.outbox.push({ to: person.me.username, kind: 'code', secret: person.mailed });
      }
    }
    return { status: 202 };
  }

  refresh(cookies: Record<string, string>, csrf: string | undefined): Answer {
    if (csrf !== '1') return refuse(403, 'auth.csrf', 'the request lacks its header');
    const username = this.sessions.get(cookies.orch_session ?? '');
    const person = username === undefined ? undefined : this.people.get(username);
    if (person === undefined) return refuse(401, 'auth.required', 'no session');
    if (person.suspended) return refuse(401, 'auth.suspended', 'the user is suspended');
    return { status: 200, body: this.grant(person, cookies.orch_session ?? '') };
  }

  logout(cookies: Record<string, string>, csrf: string | undefined): Answer {
    if (csrf !== '1') return refuse(403, 'auth.csrf', 'the request lacks its header');
    const session = cookies.orch_session ?? '';
    const username = this.sessions.get(session);
    if (username === undefined) return refuse(401, 'auth.required', 'no session');
    this.forgetSession(session);
    return { status: 204, cookies: { orch_session: null } };
  }

  /**
   * Registers somebody as the real API does. A closed system takes nobody, invited or not. Accepting names an
   * invitation, and one to accept that is no longer good is refused. A team invitation admits the registration only
   * when its acceptance is asked for, a system admission link by itself, and the admitted invitation is redeemed in the
   * same act: the reply carries where it leads when acceptance was asked for. A personal org is made only where the
   * system's settings make one.
   */
  register(body: Record<string, unknown>): Answer {
    const token = typeof body.invitation === 'string' ? body.invitation.trim() : '';
    const accept = body.accept_invitation === true;
    if (token === '' && accept) {
      return refuse(400, 'request.invalid', 'an invitation is needed to accept one', { errors: [{ path: 'invitation', message: 'is required when accepting an invitation' }] });
    }
    const offer = token === '' ? null : this.registrationOffer(token);
    if (accept && offer === null) return refuse(403, 'registration.invite_required', 'the invitation to accept is no longer available');
    const admitted = offer !== null && (accept || offer.kind === 'system') ? offer : null;
    if (this.meta.registration_mode === 'closed') return refuse(403, 'registration.closed', 'registration is closed');
    if (this.meta.registration_mode === 'invite' && admitted === null) {
      return refuse(403, 'registration.invite_required', 'an invitation that is still good is needed');
    }
    const bound = admitted?.email ?? null;
    const mustConfirm = this.confirmAddresses && bound === null;
    const given = this.identity(body, mustConfirm, mustConfirm);
    if (bound !== null && given.email !== null && given.email.toLowerCase() !== bound.toLowerCase()) {
      return refuse(403, 'registration.invite_required', 'this invitation is for another address');
    }
    const email = given.email ?? bound;
    if (mustConfirm && email !== null && this.find(email) !== undefined) return {status:202,body:{verification_required:true}};
    const n = this.people.size + 10;
    const home = this.admin.personalHome();
    const person = this.addPerson(
      fixtures.me(n, given.username, given.display_name, { email, email_verified: bound !== null, ...(home.enabled ? {} : { personal_org_id: null }) }),
      given.password,
    );
    const own = this.orgs.find((org) => org.id === person.me.personal_org_id);
    if (own !== undefined) { own.closed = home.closed; own.flat = home.flat; }
    // The invitation was found good above and the person is new, so taking it cannot be refused here.
    const accepted = admitted === null ? null : this.accept(person, { token });
    if (accepted !== null && accepted.status !== 200) throw new Error(`the double could not redeem ${token}: ${String(accepted.status)}`);
    if (mustConfirm) {
      const secret = `confirm-${randomUUID()}`;
      this.confirmations.set(secret, person.me.username);
      this.outbox.push({ to: person.me.username, kind: 'confirm', secret });
      return { status: 202, body: { verification_required: true } };
    }
    const signed = this.signedIn(person.me.username);
    return { ...signed, status: 201, body: { ...(signed.body as AuthResult), accepted_invitation: accept && accepted !== null ? accepted.body : null } };
  }

  /** An invitation that still admits somebody, from either place the double keeps them: its kind and the address a personal link is for. */
  private registrationOffer(token: string): { kind: InvitationPreview['kind']; email: string | null } | null {
    for (const list of this.places.invitations.values()) {
      const found = list.find((record) => record.token === token);
      if (found === undefined) continue;
      const invitation = found.invitation;
      const good = !invitation.revoked && invitation.uses < invitation.max_uses && Date.parse(invitation.expires_at) > Date.now();
      return good ? { kind: 'org', email: invitation.email ?? null } : null;
    }
    const invited = this.invitations.get(token);
    if (invited === undefined || invited.spent || Date.parse(invited.preview.expires_at) <= Date.now()) return null;
    return { kind: invited.preview.kind, email: invited.email };
  }

  recovery(body: Record<string, unknown>): Answer {
    const person = typeof body.email === 'string' ? this.find(body.email) : undefined;
    if (person !== undefined && this.meta.mail) {
      const secret = `reset-${randomUUID()}`;
      this.resets.set(secret, person.me.username);
      this.outbox.push({ to: person.me.username, kind: 'reset', secret });
    }
    return { status: 202 };
  }

  recoveryCheck(token: string): Answer {
    const username = this.resets.get(token);
    return username !== undefined && this.people.has(username)
      ? { status: 204 }
      : refuse(410, 'gone', 'the link was used or has ended');
  }

  recoveryConfirm(body: Record<string, unknown>): Answer {
    const token = typeof body.token === 'string' ? body.token : '';
    const password = typeof body.password === 'string' ? body.password : '';
    const username = this.resets.get(token);
    const person = username === undefined ? undefined : this.people.get(username);
    if (person === undefined) return refuse(410, 'gone', 'the link was used or has ended');
    if (Array.from(password).length < PASSWORD_MIN) {
      return refuse(400, 'request.invalid', 'the request is not valid', {
        errors: [{ path: 'password', message: `must be at least ${PASSWORD_MIN} characters` }],
      });
    }
    for (const [held, name] of this.resets) if (name === username) this.resets.delete(held);
    person.password = password;
    person.locked = {};
    for (const [session, name] of this.sessions) if (name === person.me.username) this.forgetSession(session);
    for (const [held, entry] of this.tokens) if (entry.username === person.me.username) this.tokens.delete(held);
    return { status: 204 };
  }

  verifyEmail(body: Record<string, unknown>): Answer {
    const token = typeof body.token === 'string' ? body.token : '';
    const username = this.confirmations.get(token);
    const person = username === undefined ? undefined : this.people.get(username);
    if (person === undefined) return refuse(410, 'gone', 'the link was used or has ended');
    this.confirmations.delete(token);
    person.me = { ...person.me, email_verified: true };
    // The link signs its person in: the cookie alone, and the page asks for a token.
    const session = `ors_${randomUUID()}`;
    this.sessions.set(session, person.me.username);
    this.sessionModes.set(session, { id: randomUUID(), asAdmin: false });
    return { status: 204, cookies: { orch_session: session } };
  }

  verifyResend(body: Record<string, unknown>): Answer {
    const person = typeof body.login === 'string' ? this.find(body.login) : undefined;
    if (person !== undefined && !person.me.email_verified) {
      const secret = `confirm-${randomUUID()}`;
      this.confirmations.set(secret, person.me.username);
      this.outbox.push({ to: person.me.username, kind: 'confirm', secret });
    }
    return { status: 202 };
  }

  updateMe(person: Person, body: Record<string, unknown>): Answer {
    const zoneReport = body.timezone_report === true;
    if (zoneReport && (typeof body.timezone !== 'string' || body.timezone_mode === 'fixed')) {
      return refuse(400, 'request.invalid', 'a device report needs a zone and device mode');
    }
    if (Object.hasOwn(body,'avatar_color') && body.avatar_color !== null && (typeof body.avatar_color !== 'number' || !Number.isInteger(body.avatar_color) || body.avatar_color < 0 || body.avatar_color >= 12)) {
      return refuse(400,'request.invalid','the request is not valid',{errors:[{path:'avatar_color',message:'must be a colour index from 0 to 11'}]});
    }
    // A new address is more than a preference: it asks for the password and waits to be confirmed.
    const address = this.settings.updateMe(person, body);
    const next = { ...person.me };
    if (body.avatar_color === null) {
      // The native default is the first eight SHA-256 bytes of the UUID, modulo its twelve colours.
      const digest=createHash('sha256').update(Buffer.from(person.me.id.replaceAll('-',''),'hex')).digest();
      next.avatar={...next.avatar,color:Number(digest.readBigUInt64BE(0)%12n)};
    } else if (typeof body.avatar_color === 'number') next.avatar={...next.avatar,color:body.avatar_color};
    if (body.theme === 'system' || body.theme === 'light' || body.theme === 'dark') next.theme = body.theme;
    if (body.density === 'comfortable' || body.density === 'compact') next.density = body.density;
    if (body.locale === 'en' || body.locale === 'zh-CN') next.locale = body.locale;
    if (zoneReport) {
      // A late browser report cannot undo the fixed choice a person made while it was in flight.
      if (next.timezone_mode === 'device' && typeof body.timezone === 'string') next.timezone = body.timezone;
    } else {
      if (typeof body.timezone === 'string') next.timezone = body.timezone;
      if (body.timezone_mode === 'device' || body.timezone_mode === 'fixed') next.timezone_mode = body.timezone_mode;
      else if (typeof body.timezone === 'string') next.timezone_mode = 'fixed';
    }
    if (typeof body.display_name === 'string' && body.display_name.trim() !== '') next.display_name = body.display_name.trim();
    person.me = next;
    return { status: address?.status ?? 200, body: next };
  }

  // ---------------------------------------------------------------- invitations

  /** A browser scenario prepares a real existing group's invitation; acceptance still uses the normal member writer. */
  inviteToChat(token: string, chatId: string, by: string): Answer {
    const inviter=this.people.get(by);
    if (inviter === undefined) return refuse(404,'not_found','no such inviter');
    const read=this.chats.handle(inviter,'GET',`/chats/${chatId}`,new URLSearchParams(),{});
    if (read === null) return refuse(404,'not_found','no such chat');
    const chat=read.body as Chat;
    if (chat.kind !== 'group') return refuse(400,'request.invalid','an invitation is for a group');
    const preview=fixtures.invitation(fixtures.userRef(inviter.me),{kind:'chat',org:null,what:{kind:'chat',name:chat.name ?? ''},role:null,permissions:[]});
    this.invitations.set(token,{preview,email:null,spent:false,orgId:null,chatId});
    return {status:200,body:preview};
  }

  lookup(token: string): Answer {
    for (const [orgId, list] of this.places.invitations) {
      const found = list.find((record) => record.token === token);
      if (found === undefined) continue;
      const invitation = found.invitation;
      if (invitation.revoked || invitation.uses >= invitation.max_uses || Date.parse(invitation.expires_at) < Date.now()) {
        return refuse(410, 'gone', 'the invitation is no longer good');
      }
      const row = this.trees.get(this.places.rootOf(orgId) ?? '')?.nodes.get(orgId);
      const preview: InvitationPreview = {
        kind: 'org',
        inviter: invitation.created_by,
        org: { name: row?.name ?? '', slug: this.places.metaOf(orgId).slug ?? '' },
        role: invitation.role,
        permissions: [],
        what: null,
        email_bound: invitation.form === 'email',
        expires_at: invitation.expires_at,
      };
      return { status: 200, body: preview };
    }
    const invited = this.invitations.get(token);
    if (invited === undefined || invited.spent || Date.parse(invited.preview.expires_at) <= Date.now()) return refuse(410, 'gone', 'the invitation is no longer good');
    return { status: 200, body: invited.preview };
  }

  accept(person: Person, body: Record<string, unknown>): Answer {
    const token = typeof body.token === 'string' ? body.token : '';
    for (const [orgId, list] of this.places.invitations) {
      const found = list.find((record) => record.token === token);
      if (found === undefined) continue;
      const invitation = found.invitation;
      if (invitation.revoked || invitation.uses >= invitation.max_uses) return refuse(410, 'gone', 'the invitation is no longer good');
      const root = this.places.rootOf(orgId) ?? orgId;
      if (this.places.isMember(person, root)) return refuse(409, 'member.exists', 'the person is a member already');
      invitation.uses += 1;
      this.places.join(person.me.username, root, invitation.role ?? 'viewer', orgId);
      const row = this.trees.get(root)?.nodes.get(orgId);
      return { status: 200, body: { org: row === undefined ? null : this.places.org(person, row), node:null, chat:null } satisfies InvitationAcceptReply };
    }
    const invited = this.invitations.get(token);
    if (invited === undefined || invited.spent) return refuse(410, 'gone', 'the invitation is no longer good');
    if (Date.parse(invited.preview.expires_at) <= Date.now()) return refuse(410,'gone','the invitation is no longer good');
    if (invited.email !== null && invited.email.toLowerCase() !== person.me.email?.toLowerCase()) return refuse(403,'registration.invite_required','this invitation is for another address');
    const inviter = [...this.people.values()].find((one) => one.me.id === invited.preview.inviter.id);
    let node: InvitationAcceptReply['node'] = null;
    let chat: ChatRef | null = null;
    if (invited.nodeId !== undefined && inviter !== undefined) {
      const nodeId=invited.nodeId;
      const permissions=invited.preview.permissions.length === 0 ? ['chat.view','chat.interact'] : invited.preview.permissions;
      this.access.handle(inviter,'PATCH',`/nodes/${nodeId}/permissions`,new URLSearchParams(),{rows:permissions.map((permission) => ({permission,add:[{holder:{kind:'user',id:person.me.id}}]}))});
      try { const shown=this.places.node(person,nodeId); node={id:shown.id,kind:shown.kind,name:shown.name,redacted:false}; }
      catch (error) { if (!(error instanceof Refusal)) throw error; }
      if (node === null) {
        const row=this.places.rowOf(nodeId);
        if (row !== undefined) {
          const answer=this.chats.handle(person,'GET',`/chats/${row.chat.id}`,new URLSearchParams(),{});
          if (answer !== null) { const shown=answer.body as Chat; chat={id:shown.id,kind:shown.kind,org_id:shown.org_id,node:shown.node,name:shown.name}; }
        }
      }
    }
    if (invited.chatId !== undefined && inviter !== undefined) {
      this.chats.handle(inviter,'PUT',`/chats/${invited.chatId}/members/${person.me.id}`,new URLSearchParams(),{});
      const answer=this.chats.handle(person,'GET',`/chats/${invited.chatId}`,new URLSearchParams(),{});
      if (answer !== null) { const shown=answer.body as Chat; chat={id:shown.id,kind:shown.kind,org_id:shown.org_id,node:shown.node,name:shown.name}; }
    }
    if (invited.email !== null) invited.spent = true;
    if (invited.orgId !== null) this.places.join(person.me.username, invited.orgId, invited.preview.role ?? 'viewer');
    return { status: 200, body: { org: invited.orgId === null ? null : this.orgBySlug(person, this.places.metaOf(invited.orgId).slug ?? 'acme').body, node, chat } };
  }

  // ---------------------------------------------------------------- orgs and their trees

  /** The root orgs a person belongs to, their own first, as they read them: counts and what the person holds on each. */
  orgsOf(person: Person): Org[] {
    const own = this.orgs.filter((org) => org.id === person.me.personal_org_id);
    const team = this.orgs.filter((org) => org.org_kind === 'team' && this.places.isMember(person, org.id));
    return [...own, ...team].map((org) => {
      const row = this.trees.get(org.id)?.nodes.get(org.id);
      return row === undefined ? org : this.places.org(person, row);
    });
  }

  orgBySlug(person: Person, slug: string): Answer {
    const segments = slug.split('.');
    const root = this.orgsOf(person).find((org) => org.slug === segments[0]);
    if (root === undefined) return refuse(404, 'not_found', 'no such org');
    let at = root.id;
    // An org inside another is found by its address: the short names of the orgs on the way.
    for (const segment of segments.slice(1)) {
      const child = [...(this.trees.get(root.id)?.nodes.values() ?? [])].find(
        (node) => node.kind === 'org' && node.parent_id === at && !node.deleted && (this.places.metaOf(node.id).slug ?? '') === segment,
      );
      if (child === undefined) return refuse(404, 'not_found', 'no such org');
      at = child.id;
    }
    const row = this.trees.get(root.id)?.nodes.get(at);
    if (row === undefined) return refuse(404, 'not_found', 'no such org');
    this.places.need(person, at, 'org.view');
    return { status: 200, body: this.places.org(person, row) };
  }

  tree(person: Person, orgId: string, since: number | null, query = new URLSearchParams()): Answer {
    const tree = this.trees.get(orgId);
    if (tree === undefined || !this.orgsOf(person).some((org) => org.id === orgId)) return refuse(404, 'not_found', 'no such org');
    // What the caller may see of it, each row with the documents beside it that they may read; a row that changed and is not theirs to see says only that it is gone.
    if (since !== null && (query.has('cursor') || query.has('limit'))) return refuse(400, 'request.invalid', 'since cannot be combined with a full-load cursor or limit');
    const limit = Number(query.get('limit') ?? 5000);
    if (!Number.isInteger(limit) || limit < 1 || limit > 5000) return refuse(400, 'request.invalid', 'limit must be between 1 and 5000');
    let seq = tree.seq;
    let after: string | null = null;
    const cursor = query.get('cursor');
    if (cursor !== null) {
      const parts = cursor.split('|');
      if (parts.length !== 4 || parts[0] !== orgId || parts[1] !== person.me.id || !Number.isSafeInteger(Number(parts[2])) || Number(parts[2]) < 0 || parts[3] === undefined) return refuse(400, 'request.invalid', 'cursor is not of this tree');
      seq = Number(parts[2]);
      after = parts[3];
    }
    const candidates = [...tree.nodes.values()].filter((node) => since === null ? !node.deleted && (after === null || node.id > after) : node.tree_seq > since).sort((left, right) => left.id.localeCompare(right.id));
    if (since !== null && candidates.length > 5000) return { status: 200, body: { seq, since, complete: false, nodes: [] } satisfies OrgTreeGetReply };
    const page = since === null ? candidates.slice(0, limit) : candidates;
    const last = page.at(-1);
    const next_cursor = since === null && candidates.length > limit && last !== undefined ? `${orgId}|${person.me.id}|${seq}|${last.id}` : null;
    const nodes = this.places.rowsFor(person, page, since);
    // Every row the double gives is readable: it keeps no private place that its reader administers without viewing.
    const body: OrgTreeGetReply = { nodes: nodes.map((node) => ({ ...node, content_hidden: false as const })), seq, since, complete: next_cursor === null, next_cursor };
    return { status: 200, body };
  }

  /**
   * Changes nodes of a tree, as the real system does when something happens in it: each changed row
   * is stamped with the next sequence, and whoever follows the tree is sent the rows, unless the
   * frame is to be "lost".
   */
  changeTree(orgId: string, changes: (Partial<TreeNode> & { id: string })[], lose = false): number {
    const tree = this.trees.get(orgId);
    if (tree === undefined) return 0;
    const prev = tree.seq;
    const rows: TreeNode[] = [];
    for (const change of changes) {
      const held = tree.nodes.get(change.id);
      const base: TreeNode | undefined =
        held ??
        (change.kind !== undefined && change.name !== undefined
          ? {
              id: change.id,
              kind: change.kind,
              name: change.name,
              address: null,
              parent_id: orgId,
              color: null,
              docs: 0,
              archived: false,
              deleted: false,
              restricted: false,
              participating: true,
              chat: { id: randomUUID(), seq: 0 },
              thread: null,
              topic: null,
              tree_seq: 0,
            }
          : undefined);
      if (base === undefined) continue;
      tree.seq += 1;
      const row: TreeNode = { ...base, ...change, tree_seq: tree.seq };
      tree.nodes.set(row.id, row);
      rows.push(row);
    }
    if (!lose && rows.length > 0) {
      for (const [socket, sub] of this.subscribers.get(`org:${orgId}`) ?? []) {
        // Each reader is sent the rows as they may see them; a connection that never said who reads it is sent them as they are.
        const reader = this.places.readerOf(socket);
        const nodes = reader === undefined ? rows : this.places.rowsFor(reader, rows, prev);
        socket.send({ op: 'sync', id: sub, feed: `org:${orgId}`, prev, seq: tree.seq, data: { nodes } });
      }
    }
    return tree.seq;
  }
}

/** A code as the real API mails them: letters and digits that are not mistaken for one another. */
function randomCode(length: number): string {
  const alphabet = '0123456789ABCDEFGHJKMNPQRSTVWXYZ';
  let code = '';
  for (let at = 0; at < length; at += 1) code += alphabet[Math.floor(Math.random() * alphabet.length)] ?? '0';
  return code;
}
