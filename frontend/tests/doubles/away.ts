/** The away projections extend the one world's existing notifications, routes, channels and preferences. */
import { createHash, randomUUID } from 'node:crypto';
import type { IncomingMessage } from 'node:http';

import type { NodeRef, Notification, NotificationChannel, NotificationPrefs, NotificationRoute } from '../../src/generated/api-types.ts';
import type { AwayNotification, AwayPrefs, AwayRoute, NotificationGroup, PushDevice, ReadResult } from '../../src/features/settings/personal/away/types.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

/** Undo shares the existing idempotency lifetime; it is not another notification retention period. */
const UNDO_MS = 24 * 60 * 60_000;
const PAGE = 50;
interface Ticket { user: string; expires: number; batch: Map<string, number> }
interface Replay { body: string; result: ReadResult }

export class AwayWorld implements Part {
  readonly world: World;
  constructor(world: World) { this.world = world; }
  outcomes = new Map<string, AwayRoute>();
  devices = new Map<string, PushDevice[]>();
  tickets = new Map<string, Ticket>();
  batches = new Map<string, string>();
  replays = new Map<string, Replay>();
  hidden = new Map<string, Set<string>>();
  removedChannels = new Set<string>();
  requests: { user: string; path: string; key: string | null; body: unknown }[] = [];
  faults = new Map<string, { status: number; delay: number; once: boolean }>();
  lostRead = false;
  lostTest = false;

  reset(): void {
    this.outcomes.clear(); this.devices.clear(); this.tickets.clear(); this.batches.clear(); this.replays.clear(); this.hidden.clear(); this.removedChannels.clear(); this.requests = []; this.faults.clear(); this.lostRead = false; this.lostTest = false;
  }

  private rows(user: string): Notification[] { return this.world.notifications.rows.get(user) ?? []; }
  private key(user: string, channelId: string): string { return `${user}:${channelId}`; }
  private channels(person: Person): NotificationChannel[] {
    const channels = this.world.notifications.handle(person, 'GET', '/notification-channels', new URLSearchParams(), {})?.body as NotificationChannel[];
    return channels.filter((channel) => !this.removedChannels.has(channel.id));
  }
  private prefs(person: Person): AwayPrefs {
    const prefs = this.world.notifications.handle(person, 'GET', '/me/notification-prefs', new URLSearchParams(), {})?.body as NotificationPrefs;
    return prefs;
  }
  private ref(id: string): NodeRef | null {
    for (const tree of this.world.trees.values()) {
      const row = tree.nodes.get(id);
      if (row !== undefined) return { id: row.id, name: row.name, kind: row.kind, redacted: false };
    }
    return null;
  }
  private readable(person: Person, id: string): boolean {
    if (this.hidden.get(person.me.username)?.has(id) === true) return false;
    for (const tree of this.world.trees.values()) {
      const row = tree.nodes.get(id);
      if (row !== undefined) return this.world.places.visibleTo(person, row);
    }
    return false;
  }
  private groupOf(person: Person, row: Notification): { key: string; place: NodeRef | null } | null {
    const id = row.node?.id ?? row.thread_id;
    if (id === null) return { key: 'other', place: null };
    if (!this.readable(person, id)) return null;
    for (const tree of this.world.trees.values()) {
      let node = tree.nodes.get(id);
      const path: typeof node[] = [];
      while (node !== undefined) { path.push(node); node = node.parent_id === null ? undefined : tree.nodes.get(node.parent_id); }
      const place = path.find((entry) => entry?.kind === 'project' && this.readable(person, entry.id)) ?? path.find((entry) => entry?.kind === 'org' && this.readable(person, entry.id)) ?? tree.nodes.get(id);
      if (place !== undefined && this.readable(person, place.id)) return { key: `place:${place.id}`, place: this.ref(place.id) };
    }
    return null;
  }
  private waiting(person: Person, row: Notification): boolean {
    if (row.approval_id !== null) {
      try {
        const reply = this.world.notifications.handle(person, 'GET', `/approvals/${row.approval_id}`, new URLSearchParams(), {});
        const approval = reply?.body as { state?: string; can_decide?: boolean } | undefined;
        return approval?.state === 'pending' && approval.can_decide === true;
      } catch { return false; }
    }
    // A route notice opens the person's existing repair card, never a fabricated decision right.
    return false;
  }
  private projection(person: Person, query: URLSearchParams, cut = this.world.notifications.seq(person.me.username)): AwayNotification[] {
    let rows = this.rows(person.me.username).filter((row) => row.inbox_seq <= cut && this.groupOf(person, row) !== null);
    if (query.get('unread') === 'true') rows = rows.filter((row) => row.read_at === null);
    if (query.has('kind')) rows = rows.filter((row) => (query.get('kind') ?? '').split(',').includes(row.kind));
    const groups = new Map<string, NotificationGroup>();
    for (const row of rows) {
      const place = this.groupOf(person, row);
      if (place === null) continue;
      const group = groups.get(place.key) ?? { ...place, unread: 0, waiting_for_me: 0, through_seq: cut };
      if (row.read_at === null) group.unread += 1;
      if (this.waiting(person, row)) group.waiting_for_me += 1;
      groups.set(place.key, group);
    }
    return rows.map((row) => {
      const place = this.groupOf(person, row);
      return { ...this.world.notifications.notice(person, row), kind: row.kind, group: place === null ? null : groups.get(place.key) ?? null, waiting_for_me: this.waiting(person, row) };
    }).sort((a, b) => {
      const priority = Number((b.group?.waiting_for_me ?? 0) > 0) - Number((a.group?.waiting_for_me ?? 0) > 0);
      if (priority !== 0) return priority;
      return Number(b.waiting_for_me) - Number(a.waiting_for_me) || Date.parse(b.created_at) - Date.parse(a.created_at) || b.id.localeCompare(a.id);
    });
  }
  private route(person: Person, channel: NotificationChannel): AwayRoute {
    const key = this.key(person.me.username, channel.id);
    const old = this.outcomes.get(key);
    const id = old?.id ?? channel.route.id ?? (this.world.notifications.routes.has(key) ? randomUUID() : null);
    const route:AwayRoute = { ...channel.route,...old,id,channel_id:channel.id,
      channel:id === null ? null : old?.channel ?? { id:channel.id,kind:channel.kind,name:channel.name,org:typeof channel.owner === 'object' ? channel.owner.org : null },
      needs:channel.route.needs,link:channel.route.link,label:channel.route.label };
    route.waiting_count = this.projection(person, new URLSearchParams('unread=true')).filter((row) => row.kind !== 'notification.route_attention' && (route.kinds.length === 0 || route.kinds.includes(row.kind))).length;
    if (old !== undefined || route.id !== null) this.outcomes.set(key, route);
    return route;
  }
  private routes(person: Person): AwayRoute[] {
    const live = this.channels(person).map((channel) => this.route(person, channel));
    const retained = [...this.outcomes].filter(([key, route]) => key.startsWith(`${person.me.username}:`) && route.health === 'channel_removed').map(([, route]) => ({ ...route, waiting_count: this.projection(person, new URLSearchParams('unread=true')).filter((row) => row.kind !== 'notification.route_attention').length }));
    return [...live, ...retained];
  }
  private summary(person: Person, scope?: string | null): Answer {
    if (scope !== undefined && scope !== null && !this.readable(person, scope)) throw new Refusal(403, 'authz.denied', 'View is required here', { required: 'node.view', missing: [], requestable: false });
    const prefs = this.prefs(person);
    const ways = this.routes(person).filter((route) => route.enabled || route.health === 'failed' || route.health === 'partial' || route.health === 'target_gone' || route.health === 'channel_removed').filter((route) => {
      const org = route.channel?.org;
      if (org === null || org === undefined || scope === null || scope === undefined) return true;
      for (const tree of this.world.trees.values()) {
        let row = tree.nodes.get(scope);
        while (row !== undefined) { if (row.id === org.id) return true; row = row.parent_id === null ? undefined : tree.nodes.get(row.parent_id); }
      }
      return false;
    }).map((route) => ({ route_id: route.id ?? '', name: route.channel?.name ?? 'Removed channel', kind: route.channel?.kind ?? '', state: route.health === 'failed' || route.health === 'partial' || route.health === 'target_gone' || route.health === 'channel_removed' || route.needs !== null ? 'needs_attention' : 'ready', last_ok_at: route.last_ok_at }));
    return { status: 200, body: { ...this.world.notifications.summary(person.me.username), unread: this.projection(person, new URLSearchParams('unread=true')).length, outside: { scope: scope === null || scope === undefined ? null : this.ref(scope), pause_until: prefs.pause_until !== null && Date.parse(prefs.pause_until) > Date.now() ? prefs.pause_until : null, ways } } };
  }
  private announce(person: Person, what: string): void { this.world.users.tell(person.me.username, { what }); }
  private read(person: Person, body: Record<string, unknown>, key: string | null): Answer {
    const user = person.me.username;
    const replayKey = `${user}:${key ?? ''}`;
    if (typeof body.undo_token === 'string') {
      if (Object.keys(body).length !== 1) throw new Refusal(400, 'request.invalid', 'Choose one read form');
      const ticket = this.tickets.get(body.undo_token);
      if (ticket?.user !== user) throw new Refusal(400, 'request.invalid', 'This Undo is not valid');
      if (ticket.expires <= Date.now()) throw new Refusal(410, 'gone', 'Its undo time ended');
      let changed = 0;
      for (const row of this.rows(user)) {
        if (ticket.batch.get(row.id) === row.inbox_seq && this.batches.get(`${user}:${row.id}`) === body.undo_token && this.groupOf(person, row) !== null) {
          row.read_at = null; row.inbox_seq = this.bump(user); this.batches.delete(`${user}:${row.id}`); changed += 1;
        }
      }
      this.announce(person, 'notifications_read');
      return { status: 200, body: { unread: this.projection(person, new URLSearchParams('unread=true')).length, changed, undo_token: null, undo_expires_at: null } satisfies ReadResult };
    }
    if (typeof body.group_key === 'string') {
      if (key === null || typeof body.through_seq !== 'number' || Object.keys(body).length !== 2) throw new Refusal(400, 'request.invalid', 'A group read needs its cut and key');
      const old = this.replays.get(replayKey);
      if (old !== undefined) {
        if (old.body !== JSON.stringify(body)) throw new Refusal(400, 'request.invalid', 'That key belongs to another read');
        return { status: 200, body: old.result };
      }
      const token = randomUUID();
      const batch = new Map<string, number>();
      for (const row of this.rows(user)) {
        if (row.read_at === null && row.inbox_seq <= body.through_seq && this.groupOf(person, row)?.key === body.group_key) {
          row.read_at = new Date().toISOString(); row.inbox_seq = this.bump(user); batch.set(row.id, row.inbox_seq); this.batches.set(`${user}:${row.id}`, token);
        }
      }
      const expires = Date.now() + UNDO_MS;
      this.tickets.set(token, { user, expires, batch });
      const result: ReadResult = { unread: this.projection(person, new URLSearchParams('unread=true')).length, changed: batch.size, undo_token: token, undo_expires_at: new Date(expires).toISOString() };
      this.replays.set(replayKey, { body: JSON.stringify(body), result });
      this.announce(person, 'notifications_read');
      if (this.lostRead) { this.lostRead = false; throw new Refusal(503, 'internal', 'Read response was lost'); }
      return { status: 200, body: result };
    }
    // Even a repeat individual read supersedes that row's earlier group mark.
    for (const row of this.rows(user)) if ((Array.isArray(body.ids) && body.ids.includes(row.id)) || (typeof body.node_id === 'string' && row.node?.id === body.node_id) || (body.chat_id !== undefined && row.chat_id === body.chat_id) || (typeof body.before === 'string' && row.created_at <= body.before)) this.batches.delete(`${user}:${row.id}`);
    const answer = this.world.notifications.handle(person, 'POST', '/notifications/read', new URLSearchParams(), body);
    this.announce(person, 'notifications_read');
    return answer ?? { status: 404 };
  }
  private bump(user: string): number { const seq = this.world.notifications.seq(user) + 1; this.world.notifications.seqs.set(user, seq); return seq; }

  async raw(person: Person, request: IncomingMessage, method: string, path: string): Promise<Answer | null> {
    if (method !== 'POST' || path !== '/notifications/read') return null;
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(chunk as Buffer);
    const body = JSON.parse(Buffer.concat(chunks).toString() || '{}') as Record<string, unknown>;
    const header = request.headers['idempotency-key'];
    const key = typeof header === 'string' ? header : null;
    this.requests.push({ user: person.me.username, path, key, body });
    await this.fault(path);
    return this.read(person, body, key);
  }
  private async fault(path: string): Promise<void> {
    const fault = this.faults.get(path);
    if (fault === undefined) return;
    if (fault.once) this.faults.delete(path);
    if (fault.delay > 0) await new Promise((resolve) => setTimeout(resolve, fault.delay));
    if (fault.status > 0) throw new Refusal(fault.status, fault.status === 403 ? 'authz.denied' : 'internal', fault.status === 403 ? 'Your notification choices cannot be read' : 'The notification service could not answer', { missing: [], requestable: false });
  }
  async handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Promise<Answer | null> {
    const relevant = path.startsWith('/notifications') || path.startsWith('/me/notification') || path.startsWith('/me/push-subscriptions') || path.startsWith('/notification-channels');
    if (!relevant) return null;
    this.requests.push({ user: person.me.username, path, key: null, body });
    await this.fault(path);
    const user = person.me.username;
    if (method === 'GET' && path === '/notifications/summary') return this.summary(person, query.get('node_id'));
    if (method === 'GET' && path === '/notifications') {
      const cursor = query.get('cursor');
      const [cutText, after] = cursor?.split('|') ?? [];
      const cut = cutText === undefined ? this.world.notifications.seq(user) : Number(cutText);
      let rows = this.projection(person, query, cut);
      if (query.get('group_by') !== 'place') rows.sort((a, b) => Date.parse(b.created_at) - Date.parse(a.created_at) || b.id.localeCompare(a.id));
      if (cursor !== null) { const at = rows.findIndex((row) => row.id === after); rows = at < 0 ? [] : rows.slice(at + 1); }
      const limit = Number(query.get('limit') ?? PAGE);
      const page = rows.slice(0, limit);
      return { status: 200, body: { items: page, next_cursor: rows.length > limit ? `${cut}|${page.at(-1)?.id ?? ''}` : null } };
    }
    if (path === '/me/notification-prefs') {
      const before = this.prefs(person);
      if (method === 'GET') return { status: 200, body: before };
      if (method === 'PUT') {
        if (typeof body.expected_revision === 'number' && body.expected_revision !== before.revision) throw new Refusal(409, 'conflict.stale_revision', 'Your notification choices changed');
        if ('pause_until' in body && body.expected_revision === undefined) throw new Refusal(400, 'request.invalid', 'The displayed revision is required');
        if (body.pause_until !== undefined && body.pause_until !== null && (typeof body.pause_until !== 'string' || !Number.isFinite(Date.parse(body.pause_until)) || Date.parse(body.pause_until) <= Date.now())) throw new Refusal(400, 'request.invalid', 'Choose a future time');
        const answer = this.world.notifications.handle(person, method, path, query, body);
        const next = { ...(answer?.body as NotificationPrefs), revision: before.revision + 1, pause_until: 'pause_until' in body ? body.pause_until as string | null : before.pause_until };
        this.world.notifications.prefs.set(user, next); this.announce(person, 'notification_prefs');
        return { status: 200, body: next };
      }
    }
    if (method === 'GET' && path === '/me/notification-routes') return { status: 200, body: this.routes(person) };
    if (method === 'GET' && path === '/notification-channels' && !query.has('org_id')) return { status: 200, body: this.channels(person).map((channel) => ({ ...channel, route: this.route(person, channel), enabled_route_count: null })) };
    if (method === 'GET' && path === '/notification-channels' && query.has('org_id')) {
      const answer = this.world.settings.handle(person, method, path, query, body);
      const channels = (answer?.body ?? []) as NotificationChannel[];
      return { status: 200, body: channels.filter((channel) => !this.removedChannels.has(channel.id)).map((channel) => ({ ...channel, enabled_route_count: [...this.world.notifications.routes].filter(([key, route]) => key.endsWith(`:${channel.id}`) && route.enabled).length })) };
    }
    const removed = /^\/notification-channels\/([^/]+)$/.exec(path)?.[1];
    if (method === 'DELETE' && removed !== undefined) {
      const affected: { person: Person; route: AwayRoute }[] = [];
      for (const target of this.world.people.values()) {
        const channel = this.channels(target).find((entry) => entry.id === removed);
        if (channel === undefined) continue;
        const route = this.route(target, channel);
        if (route.id !== null) affected.push({ person: target, route });
      }
      const answer = this.world.notifications.handle(person, method, path, query, body) ?? this.world.settings.handle(person, method, path, query, body);
      if (answer?.status !== 204) return answer;
      this.removedChannels.add(removed);
      for (const affectedRoute of affected) {
        const before = affectedRoute.route;
        const route: AwayRoute = { ...before, channel_id: null, enabled: false, health: 'channel_removed', failed_at: new Date().toISOString(), failure: { code: 'channel_removed', message: 'This channel was removed.', affected_devices: [] } };
        const key = this.key(affectedRoute.person.me.username, removed);
        this.outcomes.set(key, route);
        const previous = this.world.notifications.routes.get(key);
        if (previous !== undefined) this.world.notifications.routes.set(key, { ...previous, enabled: false });
        if (before.enabled) this.world.notifications.add(affectedRoute.person.me.username, { kind: 'notification.route_attention', title: `Delivery could not be confirmed through ${route.channel?.name ?? 'Removed channel'}`, body: route.failure?.message ?? '', url: `/settings/notifications#route-${route.id ?? ''}`, data: { route_id: route.id, way: route.channel?.name ?? 'Removed channel', reason: route.failure?.message ?? '', last_ok_at: route.last_ok_at, waiting_count: route.waiting_count, affected_devices: [] } });
        this.announce(affectedRoute.person, 'route');
      }
      return answer;
    }
    const routeId = /^\/me\/notification-routes\/by-id\/([^/]+)$/.exec(path)?.[1];
    if (method === 'DELETE' && routeId !== undefined) {
      const own = [...this.outcomes].find(([key, route]) => key.startsWith(`${user}:`) && route.id === routeId);
      if (own === undefined) throw new Refusal(404, 'not_found', 'No owned route');
      this.outcomes.delete(own[0]); this.world.notifications.routes.delete(own[0]); this.announce(person, 'route'); return { status: 204 };
    }
    if (method === 'PUT' && /^\/me\/notification-routes\/[^/]+$/.test(path)) {
      const channelId = path.split('/').at(-1) ?? '';
      const channel = this.channels(person).find((entry) => entry.id === channelId);
      if (channel === undefined) throw new Refusal(404, 'not_found', 'This channel was removed');
      const before = this.route(person, channel);
      if (body.retry_failed === true && (!before.enabled || before.needs !== null)) throw new Refusal(409, 'route.not_ready', 'Fix this way and switch it on first', { needs: before.needs });
      const answer = this.world.notifications.handle(person, method, path, query, body);
      const next = { ...before, ...(answer?.body as NotificationRoute), id: before.id ?? randomUUID() };
      this.outcomes.set(this.key(user, channelId), next); this.announce(person, 'route'); return { status: 200, body: next };
    }
    if (method === 'GET' && path === '/me/push-subscriptions') return { status: 200, body: this.devices.get(user) ?? [] };
    if (method === 'POST' && path === '/me/push-subscriptions') {
      const answer = this.world.notifications.handle(person, method, path, query, body);
      const hash = createHash('sha256').update(String(body.endpoint)).digest('hex');
      const devices = this.devices.get(user) ?? [];
      if (!devices.some((device) => device.endpoint_hash === hash)) devices.push({ id: randomUUID(), label: 'This browser', endpoint_hash: hash, last_ok_at: null });
      this.devices.set(user, devices); this.announce(person, 'route'); return answer;
    }
    if (method === 'DELETE' && path.startsWith('/me/push-subscriptions/')) {
      const id = path.split('/').at(-1);
      const devices = this.devices.get(user) ?? [];
      if (!devices.some((device) => device.id === id)) throw new Refusal(404, 'not_found', 'No owned device');
      this.devices.set(user, devices.filter((device) => device.id !== id));
      if (this.devices.get(user)?.length === 0) this.world.notifications.pushEndpoints.delete(user);
      this.announce(person, 'route'); return { status: 204 };
    }
    const testing = /^\/notification-channels\/([^/]+)\/test$/.exec(path)?.[1];
    if (method === 'POST' && testing !== undefined) {
      const channel = this.channels(person).find((entry) => entry.id === testing);
      if (channel === undefined) return null;
      if (channel.route.needs !== null) throw new Refusal(409, 'route.not_ready', 'Connect this way first');
      if (typeof body.device_id === 'string' && !(this.devices.get(user) ?? []).some((device) => device.id === body.device_id)) throw new Refusal(404, 'not_found', 'No owned device');
      const native = this.world.notifications.handle(person, method, path, query, body)
        ?? this.world.settings.handle(person, method, path, query, body);
      if (native === null) return null;
      const result = native.body as { ok: boolean; error: string | null } | undefined;
      if (result?.ok === false) return native;
      const route = this.route(person, channel);
      route.last_ok_at = new Date().toISOString();
      this.outcomes.set(this.key(user, channel.id), route);
      if (typeof body.device_id === 'string') for (const device of this.devices.get(user) ?? []) if (device.id === body.device_id) device.last_ok_at = route.last_ok_at;
      this.announce(person, 'route');
      if (this.lostTest) { this.lostTest = false; throw new Refusal(503, 'internal', 'Test reply was lost after acceptance'); }
      return { status: 200, body: { ok: true, error: null } };
    }
    return null;
  }
  control(path: string, body: Record<string, unknown>): Answer | null {
    const user = typeof body.user === 'string' ? body.user : 'ada';
    const person = this.world.people.get(user);
    if (person === undefined) throw new Refusal(404, 'not_found', 'No person');
    if (path === '/state') return { status: 200, body: { requests: this.requests, routes: this.routes(person), devices: this.devices.get(user) ?? [], prefs: this.prefs(person), rows: this.projection(person, new URLSearchParams()) } };
    if (path === '/fault') { this.faults.set(String(body.path), { status: Number(body.status ?? 0), delay: Number(body.delay ?? 0), once: body.once === true }); return { status: 204 }; }
    if (path === '/fault-clear') { this.faults.clear(); return { status: 204 }; }
    if (path === '/lost-read') { this.lostRead = true; return { status: 204 }; }
    if (path === '/lost-test') { this.lostTest = true; return { status: 204 }; }
    if (path === '/undo-expire') { for (const ticket of this.tickets.values()) ticket.expires = 0; return { status: 204 }; }
    if (path === '/hide') { this.hidden.set(user, new Set(Array.isArray(body.ids) ? body.ids as string[] : [])); this.announce(person, 'approvals'); return { status: 204 }; }
    if (path === '/prefs') { const prefs = { ...this.prefs(person), ...(body.prefs as object), revision: this.prefs(person).revision + 1 }; this.world.notifications.prefs.set(user, prefs); this.announce(person, 'notification_prefs'); return { status: 200, body: prefs }; }
    if (path === '/device') {
      const device = { id: randomUUID(), label: typeof body.label === 'string' ? body.label : 'Another device', endpoint_hash: createHash('sha256').update(String(body.endpoint)).digest('hex'), last_ok_at: null };
      this.devices.set(user, [...(this.devices.get(user) ?? []), device]);
      const endpoints = this.world.notifications.pushEndpoints.get(user) ?? new Set<string>(); endpoints.add(String(body.endpoint)); this.world.notifications.pushEndpoints.set(user, endpoints);
      this.announce(person, 'route'); return { status: 200, body: device };
    }
    if (path === '/route') {
      const channel = this.channels(person).find((entry) => typeof body.channel_id === 'string' ? entry.id === body.channel_id : entry.kind === body.kind);
      if (channel === undefined) throw new Refusal(404, 'not_found', 'No channel of that kind');
      const before = this.route(person, channel);
      const route = { ...before, id: before.id ?? randomUUID(), ...(body.route as Partial<AwayRoute>) };
      this.outcomes.set(this.key(user, channel.id), route);
      if (route.health === 'channel_removed') { route.enabled = false; route.channel_id = null; this.removedChannels.add(channel.id); }
      else this.world.notifications.routes.set(this.key(user, channel.id), { ...channel.route, enabled: route.enabled, label: route.label });
      if (route.health === 'failed' || route.health === 'partial' || route.health === 'target_gone' || route.health === 'channel_removed') {
        if (before.health === 'never' || before.health === 'ok') this.world.notifications.add(user, { kind: 'notification.route_attention', title: `Delivery could not be confirmed through ${channel.name}`, body: route.failure?.message ?? '', url: `/settings/notifications#route-${route.id}`, data: { route_id: route.id, way: channel.name, reason: route.failure?.message ?? '', last_ok_at: route.last_ok_at, waiting_count: route.waiting_count, affected_devices: route.failure?.affected_devices ?? [] } });
      }
      this.announce(person, 'route'); return { status: 200, body: route };
    }
    return null;
  }
}
