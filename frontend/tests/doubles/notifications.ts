/** What the fake API says of a person's notifications, the approvals that wait for them, how they are told, and the feed that keeps the first two current.
 *
 * Each person has an Inbox: rows with a number that only grows (a read mark restamps the row), counters
 * over them, and a live feed that sends what changed after a number. Approvals of the three kinds (the
 * harness ones belong to the thread double; capability and access ones are made here) are listed by
 * scope and state, decided once, and announced to everybody who is connected. The ways a person is told
 * (a mail server, a bot, push, webhooks) are channels with a route each, off until switched on. The
 * end-to-end tests steer all of it through `/__double/notify`, `/approval/*` and `/route/*`.
 */
import type {
  AccessRequest,
  Approval,
  Notification,
  NotificationChannel,
  NotificationKind,
  NotificationPrefs,
  NotificationRoute,
  TreeWaiting,
  UserRef,
} from '../../src/generated/api-types.ts';
import { id as fixtureId, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** A webhook of a person's own: it belongs to them alone. The channels of the system and of the orgs are the settings double's. */
interface ChannelDef {
  id: string;
  kind: string;
  name: string;
  user: string;
  enabled: boolean;
  config: unknown;
  secretsSet: string[];
  lastOkAt: string | null;
  lastError: string | null;
}

const PAGE = 50;

/** Fixture transport can name the new notice before the train regenerates its contract union. */
export function noticeKind(kind: string): NotificationKind { return kind as NotificationKind; }

/** Newest first, as the real list: by the moment written, then by the identifier. */
function newest(a: Pick<Notification, 'created_at' | 'id'>, b: Pick<Notification, 'created_at' | 'id'>): number {
  const delta = Date.parse(b.created_at) - Date.parse(a.created_at);
  if (delta !== 0) return delta;
  return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
}

export class NotificationWorld implements Part {
  world: World;
  rows = new Map<string, Notification[]>();
  seqs = new Map<string, number>();
  prefs = new Map<string, NotificationPrefs>();
  /** The webhooks people made for themselves. */
  channels: ChannelDef[] = [];
  /** The kinds of channel the system has, when a test narrowed them: none of the others is offered to anybody. */
  only: Set<string> | null = null;
  routes = new Map<string, NotificationRoute>();
  pushEndpoints = new Map<string, Set<string>>();
  /** Approvals made here: capability and access ones. The harness ones are the thread double's. */
  approvals = new Map<string, Approval>();
  /** Who may decide each approval made here, and who would see it only under "everything I could decide". */
  deciders = new Map<string, Set<string>>();
  also = new Map<string, Set<string>>();
  made = 0;
  madeApprovals = 0;

  constructor(world: World) {
    this.world = world;
  }

  /** The state a test begins from: no notification, no approval of this part, no webhook of anybody's own. */
  seedTeam(): void {
    this.channels = [];
    this.only = null;
  }

  reset(): void {
    this.rows.clear();
    this.seqs.clear();
    this.prefs.clear();
    this.routes.clear();
    this.pushEndpoints.clear();
    this.approvals.clear();
    this.deciders.clear();
    this.also.clear();
    this.made = 0;
    this.madeApprovals = 0;
    this.seedTeam();
  }

  // ---------------------------------------------------------------- the Inbox

  private inbox(username: string): Notification[] {
    let held = this.rows.get(username);
    if (held === undefined) {
      held = [];
      this.rows.set(username, held);
    }
    return held;
  }

  /** How far the person's Inbox has got: the number of its last row. */
  seq(username: string): number {
    return this.seqs.get(username) ?? 0;
  }

  /** What a connection that was away since a number missed: the rows after it, and the counters as they stand. */
  missed(username: string, since: number): Record<string, unknown> {
    const person = this.world.people.get(username);
    return { notifications: this.inbox(username).filter((row) => row.inbox_seq > since).map((row) => person === undefined ? row : this.notice(person, row)), summary: this.summaryOf(username) };
  }

  /** Current names use the same fixture authority as node reads, not a stored title or another browser's trees. */
  notice(person: Person, row: Notification): Notification {
    const reference = (id: string | null): Notification['node'] => {
      const node = id === null ? undefined : this.world.places.rowOf(id);
      return node === undefined ? null : { id: node.id, kind: node.kind,
        name: this.world.places.visibleTo(person, node) ? node.name : null,
        redacted: !this.world.places.visibleTo(person, node) };
    };
    const target = row.thread_id ?? row.node?.id ?? null;
    const path = target === null ? [] : this.world.places.pathTo(target).flatMap((node) => {
      const ref = reference(node.id);
      return ref === null ? [] : [ref];
    });
    const data = typeof row.data === 'object' && row.data !== null ? row.data as Record<string, unknown> : {};
    const approval = row.approval_id === null ? undefined : this.approvals.get(row.approval_id)
      ?? this.world.threads.approvals.get(row.approval_id)
      ?? this.world.access.requests.all().find((record) => record.approval.id === row.approval_id)?.approval;
    const actorId = typeof data.actor === 'string' ? data.actor : typeof data.by === 'string' ? data.by : null;
    const supplied = actorId === null ? undefined : [...this.world.people.values()].find((by) => by.me.id === actorId)?.me;
    const requester = approval?.requester;
    const requestingPerson = requester !== undefined && 'user' in requester ? requester.user : null;
    const settled = approval !== undefined && approval.state !== 'pending'
      && (row.kind === 'approval.requested' || row.kind === 'capability.requested' || row.kind === 'access.requested');
    const actor = settled ? approval.decided_by : row.actor ?? (supplied === undefined ? undefined : userRef(supplied))
      ?? (row.kind === 'access.decided' ? approval?.decided_by : requestingPerson) ?? null;
    const choice=typeof approval?.decision==='object'&&approval.decision!==null&&!Array.isArray(approval.decision)?approval.decision as Record<string,unknown>:{};
    const current: Record<string, unknown> = settled ? { ...data, approval_state: approval.state, decided_by: approval.decided_by?.id ?? null,
      decided_at:approval.decided_at,request_kind:approval.kind,
      request_decision:{...(typeof choice.expires_at==='string'||choice.expires_at===null?{expires_at:choice.expires_at}:{}),...(typeof choice.note==='string'?{note:choice.note}:{})},
    } : data;
    const parameters: Record<string, unknown> = { ...current };
    delete parameters._orch_action;
    const details = settled && (row.details === null || row.details === undefined || row.details.v === 1)
      ? { v: 1, action: row.kind, parameters } : row.details;
    if (settled && details?.v === 1) current._orch_action = details;
    const url = row.approval_id !== null && row.url === '/approvals' ? `/approvals?approval=${encodeURIComponent(row.approval_id)}` : row.url;
    return { ...row, ...(settled ? { data: current, details } : {}), node: reference(row.node?.id ?? null), thread: reference(row.thread_id), path, actor, url };
  }

  private next(username: string): number {
    const number = this.seq(username) + 1;
    this.seqs.set(username, number);
    return number;
  }

  /** The two counters, as the server counts them for this person. */
  summary(username: string): { unread: number; approvals_pending: number; seq: number; outside: { scope: null; pause_until: string | null; ways: [] } } {
    const person = this.world.people.get(username);
    return {
      unread: this.inbox(username).filter((row) => row.read_at === null).length,
      approvals_pending: person === undefined ? 0 : this.listApprovals(person, 'mine', 'pending', null, false).filter((approval) => approval.can_decide).length,
      seq: this.seq(username),
      outside: { scope: null, pause_until: this.prefsOf(username).pause_until, ways: [] },
    };
  }

  /** Writes a row into somebody's Inbox, as the system would, and tells the connections that follow it. */
  add(username: string, more: Partial<Notification> & { kind: NotificationKind; title: string }, lose = false): Notification {
    this.made += 1;
    const seq = this.next(username);
    const node = more.node ?? null;
    const row: Notification = {
      id: fixtureId(11, this.made),
      kind: more.kind,
      title: more.title,
      body: more.body ?? '',
      node,
      thread_id: more.thread_id ?? null,
      approval_id: more.approval_id ?? null,
      chat_id: more.chat_id ?? null,
      url: more.url ?? (more.approval_id !== undefined && more.approval_id !== null ? `/approvals?approval=${encodeURIComponent(more.approval_id)}` : more.thread_id === undefined || more.thread_id === null ? '/inbox' : `/thread/${more.thread_id}`),
      data: more.data ?? {},
      details: more.details,
      created_at: more.created_at ?? new Date().toISOString(),
      read_at: more.read_at ?? null,
      inbox_seq: seq,
      group: more.group ?? null,
      waiting_for_me: more.waiting_for_me ?? false,
    };
    this.inbox(username).push(row);
    this.publish(username, [row], lose);
    const person = this.world.people.get(username);
    return person === undefined ? row : this.notice(person, row);
  }

  /** Sends a frame with these rows to the connections of a person; `lose` keeps the number and sends nothing. */
  private publish(username: string, rows: Notification[], lose = false): void {
    const seq = this.seq(username);
    for (const follower of this.world.users.listeners(username)) {
      if (!lose) {
        const person = this.world.people.get(username);
        follower.subscriber.send({ op: 'sync', id: follower.id, feed: 'user', prev: follower.last, seq, data: { notifications: rows.map((row) => person === undefined ? row : this.notice(person, row)), summary: this.summaryOf(username) } });
      }
      follower.last = seq;
    }
  }

  private summaryOf(username: string): { unread: number; approvals_pending: number } {
    const { unread, approvals_pending } = this.summary(username);
    return { unread, approvals_pending };
  }

  /** News that is no row: sent to every connection that follows a person's feed (or those of some people). */
  announce(what: string, users?: readonly string[]): void {
    if (what === 'approvals') {
      for (const username of users ?? this.world.people.keys()) this.world.users.tellAll({ what, summary: this.summary(username) }, [username]);
      return;
    }
    this.world.users.tellAll({ what }, users);
  }

  private list(person: Person, query: URLSearchParams): Answer {
    const unread = query.get('unread') === 'true';
    const limit = Number(query.get('limit') ?? PAGE);
    const cursor = query.get('cursor');
    let rows = [...this.inbox(person.me.username)].sort(newest);
    if (unread) rows = rows.filter((row) => row.read_at === null);
    if (cursor !== null) {
      const at = rows.findIndex((row) => `${row.created_at}|${row.id}` === cursor);
      rows = at === -1 ? [] : rows.slice(at + 1);
    }
    const page = rows.slice(0, limit);
    const last = page[page.length - 1];
    return { status: 200, body: { items: page.map((row) => this.notice(person, row)), next_cursor: rows.length > limit && last !== undefined ? `${last.created_at}|${last.id}` : null } };
  }

  private markRead(person: Person, body: Record<string, unknown>): Answer {
    const username = person.me.username;
    const ids = Array.isArray(body.ids) ? (body.ids as string[]) : null;
    const node = typeof body.node_id === 'string' ? body.node_id : null;
    const chat = typeof body.chat_id === 'string' ? body.chat_id : null;
    const before = typeof body.before === 'string' ? body.before : null;
    if (ids === null && node === null && chat === null && before === null) {
      throw new Refusal(400, 'request.invalid', 'at least one member is needed', { errors: [{ path: 'ids', message: 'name rows, a node, a chat or a moment' }] });
    }
    const changed: Notification[] = [];
    for (const row of this.inbox(username)) {
      if (row.read_at !== null) continue;
      const hit =
        (ids?.includes(row.id) ?? false) ||
        (node !== null && (row.node?.id === node || row.thread_id === node)) ||
        (chat !== null && row.chat_id === chat) ||
        (before !== null && Date.parse(row.created_at) <= Date.parse(before));
      if (!hit) continue;
      row.read_at = new Date().toISOString();
      row.inbox_seq = this.next(username);
      changed.push(row);
    }
    if (changed.length > 0) this.publish(username, changed);
    return { status: 200, body: { unread: this.summary(username).unread } };
  }

  // ---------------------------------------------------------------- approvals

  private sentTo(person: Person, approval: Approval): boolean {
    if (approval.kind === 'harness') {
      return this.world.places.access(person, approval.thread_id ?? '').permissions.includes('thread.manage');
    }
    if (approval.kind === 'access') {
      const record = this.world.access.requests.recordOf(approval.id);
      return record !== undefined && this.world.access.requests.mayDecide(person, record);
    }
    return this.deciders.get(approval.id)?.has(person.me.username) === true;
  }

  private couldDecide(person: Person, approval: Approval): boolean {
    if (approval.kind === 'access') {
      const record = this.world.access.requests.recordOf(approval.id);
      return record !== undefined && this.world.access.requests.couldDecide(person, record);
    }
    return this.sentTo(person, approval) || this.also.get(approval.id)?.has(person.me.username) === true;
  }

  /** An approval as a person sees it: whether they may decide it is computed for them. */
  view(person: Person, approval: Approval): Approval {
    if (approval.kind === 'access') {
      const record = this.world.access.requests.recordOf(approval.id);
      return record === undefined ? approval : this.world.access.requests.shape(person, record);
    }
    const may = approval.state === 'pending' && (approval.kind === 'harness' ? approval.can_decide && this.sentTo(person, approval) : this.deciders.get(approval.id)?.has(person.me.username) === true);
    return { ...approval, can_decide: may };
  }

  private everyApproval(): Approval[] {
    return [...this.world.threads.approvals.values(), ...this.approvals.values(), ...this.world.access.requests.all().map((record) => record.approval)];
  }

  /** The compact tree uses the very same per-reader decision authority as the ordinary cards. */
  treeWaits(person: Person, threads: ReadonlySet<string>): Map<string, TreeWaiting> {
    const waits = new Map<string, TreeWaiting>();
    for (const approval of this.everyApproval()) {
      if (approval.thread_id === null || !threads.has(approval.thread_id) || approval.state !== 'pending'
        || (approval.expires_at !== null && Date.parse(approval.expires_at) <= Date.now())) continue;
      const next: TreeWaiting = {
        kind: approval.kind === 'harness' && (approval.request.request_kind === 'user_input' || approval.request.request_kind === 'mcp_elicitation') ? 'answer' : 'approval',
        for_caller: this.view(person, approval).can_decide,
      };
      const held = waits.get(approval.thread_id);
      if (held === undefined || Number(next.for_caller) > Number(held.for_caller)
        || (next.for_caller === held.for_caller && next.kind === 'answer')) waits.set(approval.thread_id, next);
    }
    return waits;
  }

  private listApprovals(person: Person, scope: string, state: string, kind: string | null, all: boolean): Approval[] {
    const username = person.me.username;
    return this.everyApproval()
      .filter((approval) => (state === 'decided' ? approval.state !== 'pending' : approval.state === 'pending'))
      .filter((approval) => kind === null || approval.kind === kind)
      .filter((approval) => {
        if (scope === 'requested') return 'user' in approval.requester && approval.requester.user.username === username;
        if (scope.startsWith('node:')) return approval.node?.id === scope.slice('node:'.length) || approval.thread_id === scope.slice('node:'.length);
        return all ? this.couldDecide(person, approval) : this.sentTo(person, approval);
      })
      .map((approval) => this.view(person, approval))
      .sort((a, b) => newest({ created_at: a.created_at, id: a.id }, { created_at: b.created_at, id: b.id }));
  }

  private listApprovalsAnswer(person: Person, query: URLSearchParams): Answer {
    const found = this.listApprovals(person, query.get('scope') ?? 'mine', query.get('state') ?? 'pending', query.get('kind'), query.get('all') === 'true');
    const limit = Number(query.get('limit') ?? 20);
    const cursor = query.get('cursor');
    const from = cursor === null ? 0 : found.findIndex((approval) => approval.id === cursor) + 1;
    const page = found.slice(from, from + limit);
    const last = page[page.length - 1];
    return { status: 200, body: { items: page, next_cursor: from + limit < found.length && last !== undefined ? last.id : null } };
  }

  private decide(person: Person, id: string, body: Record<string, unknown>): Approval {
    const approval = this.approvals.get(id);
    if (approval === undefined) throw new Refusal(404, 'not_found', 'there is no such approval');
    if (approval.state !== 'pending') {
      throw new Refusal(409, 'approval.already_decided', 'somebody answered first', { current: this.view(person, approval) });
    }
    if (this.deciders.get(id)?.has(person.me.username) !== true) {
      throw new Refusal(403, 'authz.denied', 'you are not among those who decide this');
    }
    const approve = body.approve === true;
    const decided: Approval = { ...approval, state: approve ? 'approved' : 'denied', decision: body, decided_at: new Date().toISOString(), decided_by: userRef(person.me) };
    this.approvals.set(id, decided);
    this.settled();
    return this.view(person, decided);
  }

  /** Something left `pending`, or was made: everybody connected is told, and reads again. */
  settled(): void {
    for (const [username, rows] of this.rows) {
      const changed = rows.filter((row) => {
        const approval = row.approval_id === null ? undefined : this.approvals.get(row.approval_id)
          ?? this.world.threads.approvals.get(row.approval_id)
          ?? this.world.access.requests.all().find((record) => record.approval.id === row.approval_id)?.approval;
        const data = typeof row.data === 'object' && row.data !== null ? row.data as Record<string, unknown> : {};
        return approval !== undefined && approval.state !== 'pending' && data._orch_approval_settled !== true;
      });
      if (changed.length === 0) continue;
      const seq = this.next(username);
      for (const row of changed) {
        row.read_at ??= new Date().toISOString(); row.inbox_seq = seq; row.waiting_for_me = false;
        const data = typeof row.data === 'object' && row.data !== null ? row.data as Record<string, unknown> : {};
        row.data = { ...data, _orch_approval_settled: true };
        this.world.away.batches.delete(`${username}:${row.id}`);
      }
      this.publish(username, changed);
    }
    this.announce('approvals');
  }

  /** Makes an approval of the kind capability (kept here) or access (put into the access double, which keeps every request), as the system would when an environment or a person asks. */
  makeApproval(kind: 'capability' | 'access', more: Record<string, unknown>): Approval {
    this.madeApprovals += 1;
    const approvalId = typeof more.id === 'string' ? more.id : fixtureId(12, this.madeApprovals);
    const requesterName = typeof more.requester === 'string' ? more.requester : 'grace';
    const requester: UserRef = this.world.places.userOfName(requesterName);
    const node = (more.node ?? null) as Approval['node'];
    const base = {
      approver_scope: null,
      batch_id: typeof more.batch_id === 'string' ? more.batch_id : null,
      can_decide: false,
      created_at: typeof more.created_at === 'string' ? more.created_at : new Date().toISOString(),
      decided_at: null,
      decided_automatically: false,
      decided_by: null,
      decision: null,
      expires_at: null,
      id: approvalId,
      item_id: null,
      node,
      object: (more.object ?? null) as Approval['object'],
      requester: { user: requester },
      required_permission: kind === 'access' ? 'access.manage' : '',
      state: 'pending' as const,
      thread_id: typeof more.thread_id === 'string' ? more.thread_id : null,
      title: typeof more.title === 'string' ? more.title : kind === 'capability' ? 'GPU for Training box' : 'Access to Fix login',
    };
    const approval: Approval =
      kind === 'capability'
        ? {
            ...base,
            approver_scope: 'host',
            kind: 'capability',
            request: {
              capability: 'gpu',
              consequences: (more.consequences as string[] | undefined) ?? ['The environment is recreated with the GPU.', 'Work that runs in it stops for a minute.'],
              environment: { id: fixtureId(5, 1), name: 'Training box', address: 'acme.grace.training-box', owner: requester, ports_exposed: false, placement: 'connected', status: 'running' },
              host: { id: fixtureId(6, 1), name: "Grace's laptop", address: 'acme.grace.laptop' },
              label: 'GPU',
              reason: typeof more.reason === 'string' ? more.reason : 'Training a model needs the GPU.',
              risk: (more.risk as 'low' | 'medium' | 'high' | 'critical' | undefined) ?? 'high',
            },
          }
        : {
            ...base,
            kind: 'access',
            request: {
              for_thread: (more.for_thread ?? null) as AccessRequest['for_thread'],
              holder: { kind: 'user', user: requester },
              options: more.options ?? null,
              permissions: (more.permissions as string[] | undefined) ?? ['thread.view', 'thread.interact'],
              reason: typeof more.reason === 'string' ? more.reason : 'I would like to talk to the agent.',
            },
          };
    const deciders = (more.deciders as string[] | undefined) ?? ['ada'];
    const also = (more.also as string[] | undefined) ?? [];
    if (kind === 'access') {
      const asking = this.world.people.get(requesterName);
      if (asking === undefined) throw new Refusal(404, 'not_found', 'there is no such person');
      this.world.access.requests.adopt(approval, asking, { deciders, also });
    } else {
      this.approvals.set(approvalId, approval);
      this.deciders.set(approvalId, new Set(deciders));
      this.also.set(approvalId, new Set(also));
    }
    this.settled();
    return approval;
  }

  /** Settles an approval as somebody else did, behind the page's back; `announce` false keeps it quiet, so that a page that still shows it as waiting finds out only by its own decision. */
  settleAs(id: string, by: string, state: 'approved' | 'denied', decision: unknown, announce = true, at?:string): Approval {
    const asked = this.world.access.requests.recordOf(id);
    const held = this.approvals.get(id) ?? this.world.threads.approvals.get(id) ?? asked?.approval;
    if (held === undefined) throw new Refusal(404, 'not_found', 'there is no such approval');
    const decidedAt=at??new Date().toISOString();
    const decided: Approval = { ...held, state, decision, decided_at: decidedAt, decided_by: this.world.places.userOfName(by) };
    if (this.approvals.has(id)) this.approvals.set(id, decided);
    else if (asked !== undefined) this.world.access.requests.settle(id, this.world.places.userOfName(by), state, decision,decidedAt);
    else this.world.threads.approvals.set(id, decided);
    if (announce) this.settled();
    return decided;
  }

  // ---------------------------------------------------------------- how a person is told

  /** A webhook of the person's own, as they see it: all of it, and the route to it. */
  private ownChannel(person: Person, def: ChannelDef): NotificationChannel {
    return {
      can_edit: true,
      config: def.config,
      enabled: def.enabled,
      enabled_route_count: 0,
      id: def.id,
      kind: def.kind,
      last_error: def.lastError,
      last_ok_at: def.lastOkAt,
      name: def.name,
      owner: 'me',
      route: this.routeOf(person, def.id, def.kind),
      secrets_set: def.secretsSet,
      templates: {},
    };
  }

  /** A channel of the system or of an org, as a person who only routes to it sees it: nothing of how it is set up, and the route to it. */
  private sharedChannel(person: Person, channel: NotificationChannel): NotificationChannel {
    return { ...channel, can_edit: false, config: {}, enabled_route_count: null, last_error: null, route: this.routeOf(person, channel.id, channel.kind), secrets_set: [], templates: {} };
  }

  private needs(person: Person, kind: string, route: NotificationRoute): NotificationRoute['needs'] {
    if (kind === 'email') return person.me.email_verified ? null : 'verified_email';
    if (kind === 'push') return (this.pushEndpoints.get(person.me.username)?.size ?? 0) > 0 ? null : 'device';
    if (kind === 'telegram') return route.label === null ? 'link' : null;
    return null;
  }

  private routeOf(person: Person, channelId: string, kind: string): NotificationRoute {
    const key = `${person.me.username}:${channelId}`;
    const held: NotificationRoute = this.routes.get(key) ?? {
      id: null,
      channel_id: channelId,
      channel: { id: channelId, kind, name: kind, org: null },
      enabled: false,
      health: 'never',
      last_ok_at: null,
      failed_at: null,
      failure: null,
      waiting_count: 0,
      kinds: [],
      label: null,
      last_sent_at: null,
      link: null,
      min_interval_minutes: null,
      needs: null,
      unread_delay_minutes: null,
    };
    const live: NotificationRoute = { ...held, needs: null };
    live.needs = this.needs(person, kind, live);
    if (live.needs !== null) live.enabled = false;
    return live;
  }

  /** Every channel the person can route to: those of the system and of their orgs (kept by the settings double) and their own webhooks. */
  private channelsFor(person: Person): NotificationChannel[] {
    const shared = this.world.settings.channelsVisibleTo(person).filter((channel) => this.only === null || this.only.has(channel.kind));
    return [...shared.map((channel) => this.sharedChannel(person, channel)), ...this.channels.filter((def) => def.user === person.me.username).map((def) => this.ownChannel(person, def))];
  }

  private setRoute(person: Person, channelId: string, body: Record<string, unknown>): NotificationRoute {
    const channel = this.channelsFor(person).find((entry) => entry.id === channelId);
    if (channel === undefined) throw new Refusal(404, 'not_found', 'there is no such channel');
    const before = this.routeOf(person, channelId, channel.kind);
    if (body.enabled === true && before.needs !== null) {
      throw new Refusal(409, 'route.not_ready', 'the route cannot reach the person yet', { needs: before.needs });
    }
    const next: NotificationRoute = { ...before };
    if (typeof body.enabled === 'boolean') next.enabled = body.enabled;
    if (Array.isArray(body.kinds)) next.kinds = body.kinds as NotificationKind[];
    if ('unread_delay_minutes' in body) next.unread_delay_minutes = (body.unread_delay_minutes as number | null) ?? null;
    if ('min_interval_minutes' in body) next.min_interval_minutes = (body.min_interval_minutes as number | null) ?? null;
    for (const field of [next.unread_delay_minutes, next.min_interval_minutes]) {
      if (field !== null && (field < 1 || field > 1440)) {
        throw new Refusal(400, 'request.invalid', 'a delay is from one minute to a day', { errors: [{ path: 'unread_delay_minutes', message: 'is from 1 to 1440' }] });
      }
    }
    this.routes.set(`${person.me.username}:${channelId}`, next);
    return this.routeOf(person, channelId, channel.kind);
  }

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    const username = person.me.username;
    let found: RegExpExecArray | null;

    if (route === 'GET /notifications/summary') return { status: 200, body: this.summary(username) };
    if (route === 'GET /notifications') return this.list(person, query);
    if (route === 'POST /notifications/read') return this.markRead(person, body);

    if (route === 'GET /me/notification-prefs') return { status: 200, body: this.prefsOf(username) };
    if (route === 'PUT /me/notification-prefs') {
      const next = { ...this.prefsOf(username) };
      if (typeof body.approvals_always === 'boolean') next.approvals_always = body.approvals_always;
      if (body.chat_node_level === 'all' || body.chat_node_level === 'mentions' || body.chat_node_level === 'off') next.chat_node_level = body.chat_node_level;
      if (body.chat_direct_level === 'all' || body.chat_direct_level === 'mentions' || body.chat_direct_level === 'off') next.chat_direct_level = body.chat_direct_level;
      if ('quiet_hours' in body) next.quiet_hours = (body.quiet_hours as NotificationPrefs['quiet_hours']) ?? null;
      if (typeof body.expected_revision === 'number' && body.expected_revision !== next.revision) {
        throw new Refusal(409, 'conflict.stale_revision', 'your choices were changed meanwhile', { current: this.prefsOf(username) });
      }
      if ('pause_until' in body) next.pause_until = (body.pause_until as string | null) ?? null;
      next.revision += 1;
      this.prefs.set(username, next);
      return { status: 200, body: next };
    }

    // What a person can route to, without an org named; the channels of an org, and the kinds there are, are the settings double's.
    if (route === 'GET /notification-channels' && !query.has('org_id')) return { status: 200, body: this.channelsFor(person) };

    if ((found = /^PUT \/me\/notification-routes\/([^/]+)$/.exec(route))) return { status: 200, body: this.setRoute(person, found[1] ?? '', body) };
    if ((found = /^DELETE \/me\/notification-routes\/([^/]+)$/.exec(route))) {
      this.routes.delete(`${username}:${found[1] ?? ''}`);
      return { status: 204 };
    }
    if ((found = /^POST \/me\/notification-routes\/([^/]+)\/link$/.exec(route))) {
      const channel = this.channelsFor(person).find((entry) => entry.id === found?.[1]);
      if (channel?.kind !== 'telegram') throw new Refusal(404, 'not_found', 'there is no such channel');
      const link = { url: `https://t.me/acme_orch_bot?start=${fixtureId(13, this.made + 1).slice(-12)}`, expires_at: new Date(Date.now() + 10 * 60_000).toISOString() };
      this.routes.set(`${username}:${channel.id}`, { ...this.routeOf(person, channel.id, channel.kind), link });
      return { status: 200, body: link };
    }
    if ((found = /^POST \/notification-channels\/([^/]+)\/test$/.exec(route))) {
      const def = this.channels.find((entry) => entry.id === found?.[1] && entry.user === username);
      if (def === undefined) return null;
      // A far side that answers badly: an address with "fail" in it.
      const address = (def.config as { url?: unknown } | null)?.url;
      const refused = typeof address === 'string' && address.includes('fail');
      return { status: 200, body: { ok: !refused, error: refused ? 'the far side answered 502' : null } };
    }
    if (route === 'POST /me/notification-channels') {
      const config = (body.config ?? {}) as { url?: string };
      if (typeof body.name !== 'string' || body.name.trim() === '') {
        throw new Refusal(400, 'request.invalid', 'a name is needed', { errors: [{ path: 'name', message: 'is needed' }] });
      }
      if (typeof config.url !== 'string' || !/^https?:\/\//.test(config.url)) {
        throw new Refusal(400, 'request.invalid', 'the address is not one', { errors: [{ path: 'config.url', message: 'is not a web address' }] });
      }
      const named = body.name.trim();
      if (this.channels.some((entry) => entry.user === username && entry.name === named)) {
        throw new Refusal(400, 'request.invalid', 'the name is used already', { errors: [{ path: 'name', message: 'is used already' }] });
      }
      this.made += 1;
      const def: ChannelDef = {
        id: fixtureId(10, 100 + this.made),
        kind: 'webhook',
        name: body.name,
        user: username,
        enabled: true,
        config: { url: config.url },
        secretsSet: body.secrets !== null && typeof body.secrets === 'object' && 'secret' in body.secrets ? ['secret'] : [],
        lastOkAt: null,
        lastError: null,
      };
      this.channels.push(def);
      return { status: 200, body: this.ownChannel(person, def) };
    }
    if ((found = /^PATCH \/notification-channels\/([^/]+)$/.exec(route))) {
      const def = this.channels.find((entry) => entry.id === found?.[1] && entry.user === username);
      if (def === undefined) return null;
      if (typeof body.name === 'string') def.name = body.name;
      if (body.config !== null && typeof body.config === 'object') def.config = body.config;
      if (typeof body.enabled === 'boolean') def.enabled = body.enabled;
      return { status: 200, body: this.ownChannel(person, def) };
    }
    if ((found = /^DELETE \/notification-channels\/([^/]+)$/.exec(route))) {
      const at = this.channels.findIndex((entry) => entry.id === found?.[1] && entry.user === username);
      if (at === -1) return null;
      this.channels.splice(at, 1);
      this.routes.delete(`${username}:${found[1] ?? ''}`);
      return { status: 204 };
    }

    if (route === 'POST /me/push-subscriptions') {
      const endpoint = typeof body.endpoint === 'string' ? body.endpoint : '';
      const held = this.pushEndpoints.get(username) ?? new Set<string>();
      held.add(endpoint);
      this.pushEndpoints.set(username, held);
      return { status: 204 };
    }
    if (route === 'DELETE /me/push-subscriptions') {
      this.pushEndpoints.get(username)?.delete(typeof body.endpoint === 'string' ? body.endpoint : '');
      return { status: 204 };
    }

    // One list of what waits for a person, of the three kinds; each approval is read and decided where it is kept.
    if (route === 'GET /approvals') return this.listApprovalsAnswer(person, query);
    if ((found = /^GET \/approvals\/([^/]+)$/.exec(route))) {
      const own = this.approvals.get(found[1] ?? '');
      return own === undefined ? null : { status: 200, body: this.view(person, own) };
    }
    if ((found = /^POST \/approvals\/([^/]+)\/decision$/.exec(route))) {
      const id = found[1] ?? '';
      if (this.approvals.has(id)) return { status: 200, body: this.decide(person, id, body) };
      return null;
    }
    return null;
  }

  private prefsOf(username: string): NotificationPrefs {
    return this.prefs.get(username) ?? { revision: 1, pause_until: null, approvals_always: true, chat_direct_level: 'all', chat_node_level: 'mentions', quiet_hours: null };
  }

  // ---------------------------------------------------------------- the controls of a test

  /** The requests only the double has. `null` when the path is not one of this part's. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    const user = typeof body.user === 'string' ? body.user : 'ada';
    switch (path) {
      case '/row': {
        const kind = (typeof body.kind === 'string' ? body.kind : 'turn.completed') as NotificationKind;
        const row = this.add(
          user,
          {
            kind,
            ...(typeof body.actor === 'string' && this.world.people.get(body.actor) !== undefined ? { data: { actor: this.world.people.get(body.actor)?.me.id } } : {}),
            ...(typeof body.parameters === 'object' && body.parameters !== null && !Array.isArray(body.parameters) ? { details: { v: 1, action: kind, parameters: body.parameters } } : {}),
            title: typeof body.title === 'string' ? body.title : 'Main thread: turn completed',
            ...(typeof body.body === 'string' ? { body: body.body } : {}),
            ...(typeof body.url === 'string' ? { url: body.url } : {}),
            ...(typeof body.thread_id === 'string' ? { thread_id: body.thread_id } : {}),
            ...(typeof body.approval_id === 'string' ? { approval_id: body.approval_id } : {}),
            ...(typeof body.chat_id === 'string' ? { chat_id: body.chat_id } : {}),
            ...(typeof body.at === 'string' ? { created_at: body.at } : {}),
            ...(body.read === true ? { read_at: new Date().toISOString() } : {}),
            ...(typeof body.node === 'string' ? { node: this.nodeRef(body.node) } : {}),
          },
          body.lose === true,
        );
        return { status: 200, body: row };
      }
      case '/many': {
        // Rows spaced a minute apart, the newest `at` or now, so that pages and days can be looked at.
        const count = typeof body.count === 'number' ? body.count : 60;
        const end = typeof body.at === 'string' ? Date.parse(body.at) : Date.now();
        for (let index = count - 1; index >= 0; index -= 1) {
          this.add(user, {
            kind: (typeof body.kind === 'string' ? body.kind : 'turn.completed') as NotificationKind,
            title: `${typeof body.title === 'string' ? body.title : 'Notification'} ${count - index}`,
            created_at: new Date(end - index * 60_000).toISOString(),
            ...(body.read === true ? { read_at: new Date(end).toISOString() } : {}),
          });
        }
        return { status: 200, body: this.summary(user) };
      }
      case '/state':
        return { status: 200, body: { summary: this.summary(user), rows: this.inbox(user), followers: this.world.users.listeners(user).length } };
      case '/approval/capability':
        return { status: 200, body: this.makeApproval('capability', body) };
      case '/approval/access':
        return { status: 200, body: this.makeApproval('access', body) };
      case '/approval/settle': {
        const by = typeof body.by === 'string' ? body.by : 'grace';
        const state = body.state === 'denied' ? 'denied' : 'approved';
        return { status: 200, body: this.settleAs(String(body.id), by, state, body.decision ?? { approve: state === 'approved' }, body.silent !== true,typeof body.at==='string'?body.at:undefined) };
      }
      case '/approval/state':
        // What was made through this part and what was decided of it, for a test to read.
        return { status: 200, body: [...this.approvals.values(), ...this.world.access.requests.all().filter((record) => record.told !== undefined).map((record) => record.approval)] };
      case '/route/linked': {
        // The person pressed Start in the bot: the route is linked, and their feed says so.
        const asking = this.world.people.get(user);
        const def = asking === undefined ? undefined : this.channelsFor(asking).find((entry) => entry.kind === 'telegram');
        if (def === undefined) return { status: 404, body: { detail: 'no bot' } };
        const key = `${user}:${def.id}`;
        const held = this.routes.get(key);
        this.routes.set(key, {
          id: null,
          channel_id: def.id,
          channel: { id: def.id, kind: def.kind, name: def.name, org: null },
          health: 'never',
          last_ok_at: null,
          failed_at: null,
          failure: null,
          waiting_count: 0,
          enabled: false,
          kinds: [],
          last_sent_at: null,
          min_interval_minutes: null,
          unread_delay_minutes: null,
          ...held,
          needs: null,
          link: null,
          label: typeof body.label === 'string' ? body.label : '@alice_c',
        });
        this.announce('route', [user]);
        return { status: 204 };
      }
      case '/channels': {
        // Which ways out this system has: `only` names their kinds; nothing at all by default for `none`. The webhooks people made for themselves stay.
        this.only = new Set(Array.isArray(body.only) ? (body.only as string[]) : []);
        return { status: 204 };
      }
      case '/me': {
        // What a person set for themselves, put there from outside: their language, theme, density and time zone.
        const person = this.world.people.get(user);
        if (person === undefined) return { status: 404, body: { detail: 'no such person' } };
        const given: Record<string, unknown> = {};
        for (const name of ['locale', 'theme', 'density', 'timezone', 'timezone_mode']) if (name in body) given[name] = body[name];
        person.me = { ...person.me, ...given };
        return { status: 204 };
      }
      case '/mail/verified': {
        const person = this.world.people.get(user);
        if (person !== undefined) person.me = { ...person.me, email_verified: body.verified !== false };
        return { status: 204 };
      }
      case '/push/state':
        return { status: 200, body: Object.fromEntries([...this.pushEndpoints].map(([name, endpoints]) => [name, [...endpoints]])) };
      default:
        return null;
    }
  }

  private nodeRef(nodeId: string): Notification['node'] {
    const row = this.world.places.rowOf(nodeId);
    return row === undefined ? null : { id: row.id, kind: row.kind, name: row.name, redacted: false };
  }
}
