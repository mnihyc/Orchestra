/** Native view/job events of the fake API, projected by the same authorized readers as its HTTP answers. */
import { Refusal, type Answer, type Part, type Person, type Subscriber, type World } from './world.ts';
import type { Listening } from './user-feed.ts';
import type { EverythingResource, InputSchedule, McpServer, Mount, ResourceAccess, Skill } from '../../src/generated/api-types.ts';
import { ACME, userRef } from './fixtures.ts';
import { extraResourceFixtures } from './extra-resources.ts';
import { IncomingMessage } from 'node:http';
import { Socket } from 'node:net';

interface Watch {
  subscriber: Subscriber; person: Person; id: number; feed: string; path: string;
  member?: string; last?: string; timer?: ReturnType<typeof setTimeout>;
}
interface ExtraResourceRows { mcp?: Partial<McpServer>; mount?: Partial<Mount>; skill?: Pick<Skill, 'origin'> }
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const RESOURCE_PATHS: Readonly<Record<string, string>> = { provider: 'providers', agent: 'agents', machine: 'machines', host: 'hosts', template: 'templates', environment: 'environments', mcp_server: 'mcp-servers', mount: 'mounts', skill: 'skills' };

/** A simulated job worker produces its own progress; the browser never polls its job endpoint. */
const JOB_STEP_MS = 500;

export class RequestFeeds implements Part {
  private watches: Watch[] = [];
  private rows = new WeakMap<Listening, Map<string, Map<string, string>>>();
  private extraFixtures = false;
  private extraRows: ExtraResourceRows = {};
  private scheduledRows = new Map<string, { seq: number; rows: InputSchedule[] }>();
  private readonly world: World;
  constructor(world: World) { this.world = world; }

  /** One topic projection supplies HTTP openings, pending inputs and live schedule changes alike. */
  topicSchedules(person: Person, topic: string): { seq: number; rows: InputSchedule[] } {
    const held = this.scheduledRows.get(topic);
    return held === undefined ? { seq: 1, rows: [] } : { seq: held.seq, rows: held.rows.map((row) => ({ ...row,
      definition: row.author.id === person.me.id ? row.definition : null })) };
  }

  /** Native projections also serve aggregate page openings, without a second HTTP call. */
  async read(person: Person, path: string): Promise<Answer> {
    const url = new URL(path, 'http://fake');
    if (/^\/threads\/[^/]+\/live-git$/.test(url.pathname)) {
      const threadId = url.pathname.split('/')[2] ?? '';
      const reply = await this.read(person, `/threads/${threadId}/sync`);
      const body = reply.body;
      const thread = record(body) ? body.thread : undefined;
      const environment = record(thread) ? thread.environment : undefined;
      if (!record(environment) || typeof environment.id !== 'string') throw new Refusal(404, 'not_found', 'this thread has no environment');
      return this.read(person, `/environments/${environment.id}/git/status?thread=${encodeURIComponent(threadId)}`);
    }
    if (/^\/environments\/[^/]+\/live-terminals$/.test(url.pathname)) {
      const base = url.pathname.replace(/live-terminals$/, 'terminals');
      const open = await this.read(person, `${base}?state=open&limit=200`);
      const closed = await this.read(person, `${base}?state=closed&limit=10`);
      return { status: 200, body: { open: open.body, closed: closed.body } };
    }
    if (/^\/environments\/[^/]+\/sync$/.test(url.pathname)) {
      const reply = await this.read(person, url.pathname.slice(0, -5));
      const row = reply.body;
      let library: unknown = null;
      if (record(row) && record(row.access) && Array.isArray(row.access.permissions) && row.access.permissions.includes('environment.view')) {
        const facts = this.world.resources.handle(person, 'POST', '/resources/library', new URLSearchParams(), { targets: [{ object: { kind: 'environment', id: row.id }, version_id: null }] });
        if (facts === null || !Array.isArray(facts.body) || !record(facts.body[0]) || !record(facts.body[0].facts)) throw new Error('The canonical Environment Library producer must return its facts');
        library = facts.body[0].facts;
      }
      return { status: reply.status, body: { environment: row, library, seq: record(row) && typeof row.revision === 'number' ? row.revision : 1, complete: true, since: null, jobs: [] } };
    }
    if (this.extraFixtures && /^\/(?:mcp-servers|mounts|skills)(?:\/|$)/.test(url.pathname)) {
      const own = await this.handle(person, 'GET', url.pathname);
      if (own !== null) return own;
    }
    // Some native fake readers (files, terminals, processes and Git) are raw handlers. Reuse
    // their GET guard/projection with an empty in-memory request, never an extra HTTP call.
    const request=new IncomingMessage(new Socket());request.method='GET';request.url=path;
    request.push(null);
    try {
      for (const { part } of this.world.parts) {
        if (part === this) continue;
        try {
          const reply = (await part.raw?.(person,request,'GET',url.pathname,url.searchParams)) ?? (await part.handle?.(person,'GET',url.pathname,url.searchParams,{})) ?? null;
          if (reply !== null) return reply;
        } catch (error) {
          if (!(error instanceof Refusal) || error.answer.status !== 404) throw error;
        }
      }
    } finally {request.destroy();}
    throw new Refusal(404, 'not_found', 'the requested live view is not available');
  }

  private async publish(watch: Watch): Promise<void> {
    try {
      const answer = await this.read(watch.person, watch.path);
      if (!this.watches.includes(watch)) return;
      const data = watch.feed.startsWith('resource:')
        ? { what: 'changed', row: { kind: watch.feed.split(':')[1], value: answer.body } }
        : watch.member === undefined ? answer.body : { [watch.member]: answer.body };
      const measured=record(data)?{...data}:data;
      if(record(measured)) {
        delete measured.as_of;
        if(watch.feed.endsWith(':health') && record(measured.health)) {const health={...measured.health};delete health.since;measured.health=health;}
      }
      const json = JSON.stringify(measured);
      if (json !== watch.last) {
        watch.last = json;
        if (/^(?:env|topic):[^:]+$/.test(watch.feed) && record(data) && typeof data.seq === 'number') watch.subscriber.send({ op: 'sync', id: watch.id, feed: watch.feed, prev: Math.max(0, data.seq - 1), seq: data.seq, data });
        else watch.subscriber.send({ op: 'event', id: watch.id, feed: watch.feed, data });
      }
      const body = answer.body;
      if (watch.feed.startsWith('job:') && record(body) && (body.state === 'queued' || body.state === 'running')) {
        clearTimeout(watch.timer);
        watch.timer = setTimeout(() => { void this.publish(watch); }, JOB_STEP_MS);
      }
    } catch (error) {
      if (this.watches.includes(watch)) watch.subscriber.send({ op: 'error', id: watch.id, code: error instanceof Refusal && error.answer.status === 403 ? 'forbidden' : 'not_found', detail: watch.feed });
    }
  }

  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const id = Number(frame.id);
    if (frame.op === 'unsub') {
      const taken = this.watches.filter((one) => one.subscriber === subscriber && one.id === id);
      for (const one of taken) clearTimeout(one.timer);
      this.watches = this.watches.filter((one) => !taken.includes(one));
      return taken.length > 0;
    }
    if (frame.op !== 'sub' || typeof frame.feed !== 'string') return false;
    const feed = frame.feed;
    let path: string | null = null; let member: string | undefined;
    let found: RegExpExecArray | null;
    if ((found = /^usage:provider:([^:]+):health$/.exec(feed))) { path = `/usage/health?provider_id=${found[1] ?? ''}`; member = 'health'; }
    else if ((found = /^topic:([^:]+)$/.exec(feed))) path = `/topics/${found[1] ?? ''}/sync`;
    else if ((found = /^job:([^:]+)$/.exec(feed))) { path = `/jobs/${found[1] ?? ''}`; member = 'job'; }
    else if ((found = /^fork:([^:]+)$/.exec(feed))) { path = `/forks/${found[1] ?? ''}`; member = 'batch'; }
    else if ((found = /^resource:([^:]+):([^:]+)$/.exec(feed))) {
      const collection = RESOURCE_PATHS[found[1] ?? ''];
      if (collection === undefined) return false;
      path = `/${collection}/${found[2] ?? ''}`; member = 'row';
    } else if ((found = /^env:([^:]+)$/.exec(feed))) { path = `/environments/${found[1] ?? ''}/sync`; }
    else if ((found = /^env:([^:]+):mcp$/.exec(feed))) path = `/environments/${found[1] ?? ''}/mcp`;
    else if ((found = /^env:([^:]+):terminals$/.exec(feed))) path = `/environments/${found[1] ?? ''}/live-terminals`;
    else if ((found = /^env:([^:]+):procs$/.exec(feed))) path = `/environments/${found[1] ?? ''}/processes`;
    else if ((found = /^env:([^:]+):(browsers|displays)$/.exec(feed))) path = `/environments/${found[1] ?? ''}/${found[2] ?? ''}`;
    else if ((found = /^env:([^:]+):ports$/.exec(feed))) path = `/environments/${found[1] ?? ''}/ports`;
    else if ((found = /^env:([^:]+):git:(.*)$/.exec(feed))) { path = `/environments/${found[1] ?? ''}/git/status?path=${encodeURIComponent(Buffer.from(found[2] ?? '', 'base64url').toString('utf8'))}`; member = 'status'; }
    else if ((found = /^thread:([^:]+):git$/.exec(feed))) { path = `/threads/${found[1] ?? ''}/live-git`; member = 'status'; }
    else if ((found = /^node:([^:]+):model-inputs$/.exec(feed))) path = `/nodes/${found[1] ?? ''}/model-inputs`;
    else if ((found = /^thread:([^:]+):(instructions|adopt-history)$/.exec(feed))) path = `/threads/${found[1] ?? ''}/${found[2] === 'instructions' ? 'instructions' : 'adopt/history'}`;
    if (path === null) return false;
    if (person === undefined) { subscriber.send({ op: 'error', id, code: 'forbidden', detail: feed }); return true; }
    const watch: Watch = { subscriber, person, id, feed, path, ...(member === undefined ? {} : { member }) };
    this.watches.push(watch);
    subscriber.send({ op: 'subscribed', id, feed, seq: null, mode: 'ephemeral' });
    void this.publish(watch);
    return true;
  }

  gone(subscriber: Subscriber): void {
    for (const watch of this.watches.filter((one) => one.subscriber === subscriber)) clearTimeout(watch.timer);
    this.watches = this.watches.filter((one) => one.subscriber !== subscriber);
  }

  private fixtures(person: Person) {
    if (!this.extraFixtures) return null;
    const owner = this.world.people.get('ada');
    if (owner === undefined) return null;
    if (!this.world.places.access(person, ACME.org).permissions.includes('org.view')) throw new Refusal(403, 'authz.denied', 'this resource belongs to a place the caller cannot view');
    const access = (kind: string): ResourceAccess => ({ administers: person.me.username === 'ada', permissions: person.me.username === 'ada' ? [`${kind}.view`, `${kind}.use`, `${kind}.manage`] : [`${kind}.view`], through: person.me.username === 'ada' ? [{ kind: 'owner' }] : [] });
    const rows = extraResourceFixtures(userRef(owner.me), access);
    // Custom records share every transport while retaining this reader's canonical owner, home and access facts.
    const mcp = { ...rows.mcp, ...this.extraRows.mcp, owner: rows.mcp.owner, home: rows.mcp.home, org_id: rows.mcp.org_id, access: rows.mcp.access };
    const mount = { ...rows.mount, ...this.extraRows.mount, owner: rows.mount.owner, home: rows.mount.home, org_id: rows.mount.org_id, access: rows.mount.access };
    mount.locations = mount.locations.map((location) => ({ ...location, can_remove: location.can_remove && this.managesMachine(person, location.machine.id) }));
    const origin = this.extraRows.skill?.origin;
    // This delivered copy has one sanitized origin across its inventory, live detail and version records.
    if (origin !== undefined) return { ...rows, mcp, mount,
      skill: { ...rows.skill, origin, latest: rows.skill.latest === null ? null : { ...rows.skill.latest, origin: origin ?? rows.skill.latest.origin } },
      versions: { ...rows.versions, items: rows.versions.items.map((version) => ({ ...version, origin: origin ?? version.origin })) },
      files: { ...rows.files, version: { ...rows.files.version, origin } },
    };
    return { ...rows, mcp, mount };
  }

  /** Location changes use the machine's existing management facts, independently of Mount Manage. */
  private managesMachine(person: Person, id: string): boolean {
    try {
      const row = this.world.resources.handle(person, 'GET', `/machines/${id}`, new URLSearchParams(), {})?.body;
      const access = record(row) && record(row.access) ? row.access : null;
      return access !== null && Array.isArray(access.permissions) && access.permissions.some((permission: unknown) => permission === 'machine.manage' || permission === 'host.manage');
    } catch (error) { if (error instanceof Refusal) return false; throw error; }
  }

  /** The aggregate pager uses the same current-reader records as direct and live reads. */
  inventory(person: Person): EverythingResource[] {
    if (!this.extraFixtures || !this.world.places.access(person, ACME.org).permissions.includes('org.view')) return [];
    const rows = this.fixtures(person);
    return rows === null ? [] : [{ kind: 'mcp_server', value: rows.mcp }, { kind: 'mount', value: rows.mount }, { kind: 'skill', value: rows.skill }];
  }

  /** Baselines and HTTP use the same list envelope, including content visibility metadata. */
  private fixtureList(person: Person, path: string): Answer | null {
    if (!this.extraFixtures || !['/mcp-servers', '/mounts', '/skills'].includes(path)) return null;
    const rows = this.fixtures(person);
    if (rows === null) return null;
    if (path === '/mcp-servers') return { status: 200, body: [rows.mcp] };
    if (path === '/mounts') return { status: 200, body: [rows.mount] };
    return { status: 200, body: [{ ...rows.skill, content_hidden: false }] };
  }

  handle(person: Person, method: string, path: string): Answer | Promise<Answer> | null {
    if (method === 'GET' && /^\/environments\/[^/]+\/sync$/.test(path)) return this.read(person, path);
    if (!this.extraFixtures || method !== 'GET' || !/^\/(?:mcp-servers|mounts|skills)(?:\/|$)/.test(path)) return null;
    const list = this.fixtureList(person, path);
    if (list !== null) return list;
    const rows = this.fixtures(person);
    if (rows === null) return null;
    const { mcp, mount, skill, files, versions } = rows;
    if (path === `/mcp-servers/${mcp.id}`) return { status: 200, body: mcp };
    if (path === `/mounts/${mount.id}`) return { status: 200, body: mount };
    if (path === `/mounts/${mount.id}/uses`) return { status: 200, body: { scopes: [], environments: [], hidden: 0 } };
    if (path === `/skills/${skill.id}`) return { status: 200, body: skill };
    if (path === `/skills/${skill.id}/versions`) return { status: 200, body: versions };
    if (path.startsWith(`/skills/${skill.id}/versions/`)) {
      if (path.endsWith('/file')) return { status: 200, headers: { 'Content-Type': 'text/plain; charset=utf-8' }, text: '---\nname: pdf-tools\ndescription: Reads and fills PDF forms.\n---\n\n# PDF tools\n' };
      return { status: 200, body: files };
    }
    return null;
  }

  control(path: string, body: Record<string, unknown> = {}): Answer | null {
    if (path === '/topic-schedules') {
      if (typeof body.topic !== 'string' || !Array.isArray(body.rows) || !body.rows.every((row) => record(row) && typeof row.id === 'string' && record(row.author) && typeof row.author.id === 'string')) return { status: 400, body: { detail: 'Topic schedule fixtures need a topic and authored rows.' } };
      this.scheduledRows.set(body.topic, { seq: (this.scheduledRows.get(body.topic)?.seq ?? 1) + 1, rows: structuredClone(body.rows) as InputSchedule[] });
      void this.changed();
      return { status: 204 };
    }
    if (path !== '/fixtures') return null;
    for (const kind of ['mcp', 'mount'] as const) {
      const row = body[kind];
      if (row !== undefined && (!record(row) || (row.id !== undefined && typeof row.id !== 'string') || (row.name !== undefined && typeof row.name !== 'string'))) return { status: 400, body: { detail: 'A resource fixture must be a record with string identity and name fields.' } };
    }
    if (body.skill !== undefined && (!record(body.skill) || !record(body.skill.origin) || body.skill.origin.kind !== 'copy')) return { status: 400, body: { detail: 'A copied skill fixture needs its copy origin.' } };
    this.extraRows = structuredClone(body);
    this.extraFixtures = true;
    if (body.mcp !== undefined || body.mount !== undefined || body.skill !== undefined) void this.changed();
    return { status: 204 };
  }

  reset(): void {
    for (const watch of this.watches) clearTimeout(watch.timer);
    this.watches = []; this.rows = new WeakMap(); this.extraFixtures = false; this.extraRows = {}; this.scheduledRows.clear();
  }

  /** Subscription is a baseline, not a mutation of every resource on the next unrelated write. */
  prime(listener: Listening): void {
    const held = new Map<string, Map<string, string>>();
    for (const [kind, path] of Object.entries(RESOURCE_PATHS)) {
      try {
        const answer = this.fixtureList(listener.person, `/${path}`)
          ?? this.world.resources.handle(listener.person, 'GET', `/${path}`, new URLSearchParams(), {});
        const body = answer?.body;
        const rows: unknown[] = Array.isArray(body) ? body : record(body) && Array.isArray(body.items) ? body.items : [];
        held.set(kind, new Map(rows.flatMap((row) => record(row) && typeof row.id === 'string' && row.content_hidden !== true
          ? [[row.id, JSON.stringify(row)] as const] : [])));
      } catch { /* An unreadable collection supplies no baseline or news. */ }
    }
    this.rows.set(listener, held);
  }

  /** An actual fake-world mutation publishes changed native rows, separately authorized for each recipient. */
  async changed(): Promise<void> {
    for (const watch of this.watches) await this.publish(watch);
    for (const username of this.world.people.keys()) {
      for (const listener of this.world.users.listeners(username)) {
        let held = this.rows.get(listener);
        if (held === undefined) { held = new Map(); this.rows.set(listener, held); }
        for (const [kind, path] of Object.entries(RESOURCE_PATHS)) {
          let answer: Answer;
          try { answer = await this.read(listener.person, `/${path}`); } catch { continue; }
          // Authority changes retire the listener; an awaited old projection must not reach its replacement.
          if (!this.world.users.listeners(username).includes(listener)) break;
          const body = answer.body;
          const rows = Array.isArray(body) ? body as unknown[] : record(body) && Array.isArray(body.items) ? body.items as unknown[] : [];
          const previous = held.get(kind) ?? new Map<string, string>();
          const next = new Map<string, string>();
          const tell = (data: Record<string, unknown>) => listener.subscriber.send({ op: 'event', id: listener.id, feed: 'user', data });
          for (const row of rows) {
            if (!record(row) || typeof row.id !== 'string' || row.content_hidden === true) continue;
            const json = JSON.stringify(row); next.set(row.id, json);
            if (previous.get(row.id) !== json) tell({ what: 'resources', kind, id: row.id, row });
          }
          for (const id of previous.keys()) if (!next.has(id)) tell({ what: 'resources', kind, id, deleted: true });
          held.set(kind, next);
        }
      }
    }
  }
}
