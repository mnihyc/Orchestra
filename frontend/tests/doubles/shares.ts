/** What the fake API says of sharing with agents: a thing given to the agents of a thread or a place as a copy or as a share that stays, what was shared and with whom, what the agents of a place know, and taking a share back.
 *
 * It keeps to what the real API promises as far as a screen can tell: a live share is entries on the pages
 * of what is shared (the place's own page and its defaults page, in the rows of the areas named), so that the
 * page lists the agents beside the people and a share taken back leaves no entry; a copy is a record with the size
 * it would come to; both are all or nothing over the threads or places named, are checked against what the
 * sharer holds and may share, and answer the refusals a form acts on: too large, tools off, a thread that is
 * not the sharer's to drive. The areas, the places and the people are those of the other parts of the world.
 */
import { randomUUID } from 'node:crypto';

import type { AgentShare, Entry, NodeRef, ShareParts, SharePreview, SharePreviewRow, ShareWithAgents } from '../../src/generated/api-types.ts';
import type { Access, StoredPage } from './access.ts';
import { type Answer, type Person, Refusal } from './world.ts';

/** A copy of one area is this large in the fake world, and a copy may be this large unless a test says otherwise (`sharing.snapshot_max_bytes`). */
const AREA_BYTES = 5 * 1024 * 1024;
const SNAPSHOT_LIMIT = 64 * 1024 * 1024;
/** How many threads or places one request may name. */
const DESTINATIONS_MAX = 10;

/** The rows of the page that carry each area at each level. */
const ROWS = {
  timeline: { read: ['thread.view'], write: ['thread.view', 'thread.interact'] },
  chat: { read: ['chat.view'], write: ['chat.view', 'chat.interact'] },
  documents: { read: ['doc.view'], edit: ['doc.view', 'doc.edit'], manage: ['doc.view', 'doc.edit', 'doc.manage'] },
} as const;

type Area = keyof typeof ROWS;
const AREAS: readonly Area[] = ['timeline', 'chat', 'documents'];

/** The permissions a share of these parts lets the agents use, which the sharer has to hold. */
function needed(parts: ShareParts): string[] {
  const found = new Set<string>();
  const timeline = parts.timeline;
  if (timeline !== undefined && timeline !== null) for (const name of ROWS.timeline[timeline]) found.add(name);
  const chat = parts.chat;
  if (chat !== undefined && chat !== null) for (const name of ROWS.chat[chat]) found.add(name);
  const documents = parts.documents;
  if (documents !== undefined && documents !== null) for (const name of ROWS.documents[documents]) found.add(name);
  return [...found];
}

interface Carried {
  /** The page the entry stands on: a node's own page or its defaults. */
  id: string;
  defaults: boolean;
  permission: string;
  profile: string;
  entryId: string;
}

interface Share {
  id: string;
  what: { kind: 'node' | 'chat' | 'document'; id: string };
  to: string;
  mode: 'snapshot' | 'live';
  inside: boolean;
  download: boolean;
  parts: ShareParts;
  expires_at: string | null;
  by: Person;
  at: string;
  carried: Carried[];
  snapshot: AgentShare['snapshot'];
  clientId: string;
}

const is = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);

/** How many of the three areas a share names. */
const areasShared = (parts: ShareParts): number => AREAS.filter((area) => parts[area] !== undefined && parts[area] !== null).length;

export class Shares {
  private records: Share[] = [];
  /** Where the policy `sharing.agent_reach` was set, by place. */
  private reach = new Map<string, 'none' | 'topic' | 'project'>();
  /** Topics whose agents get none of Orch's own tools. */
  private toolsOff = new Set<string>();
  private snapshotLimit = SNAPSHOT_LIMIT;
  private areaBytes = AREA_BYTES;
  /** Whether copies can be kept at all; the real system answers that they cannot until the workspace can hold them. */
  copiesKept = true;
  private readonly access: Access;

  constructor(access: Access) {
    this.access = access;
  }

  reset(): void {
    this.records = [];
    this.reach.clear();
    this.toolsOff.clear();
    this.snapshotLimit = SNAPSHOT_LIMIT;
    this.areaBytes = AREA_BYTES;
    this.copiesKept = true;
  }

  // ---------------------------------------------------------------- shapes

  private nodeRef(id: string): NodeRef {
    const found = this.access.nodeRefOf(id);
    return typeof found === 'string' ? { id, kind: 'org', name: null, redacted: true } : found;
  }

  private entryOf(carried: Carried): Entry | null {
    const page = this.access.pageFor(carried.id, carried.defaults);
    return page.rows.get(`${carried.permission}|${carried.profile}`)?.entries.find((entry) => entry.id === carried.entryId) ?? null;
  }

  private shape(share: Share): AgentShare {
    const entries = share.carried.flatMap((carried) => this.entryOf(carried) ?? []);
    const body: AgentShare = {
      read_first: false,
      id: share.id,
      what: share.what.kind === 'node' ? this.nodeRef(share.what.id) : ({ id: share.what.id, name: '', redacted: false } as never),
      to: this.nodeRef(share.to),
      mode: share.mode,
      inside: share.inside,
      parts: share.parts,
      entries,
      snapshot: share.snapshot,
      input: null,
      by: this.access.userRefOf(share.by),
      at: share.at,
      download: share.download,
    };
    return body;
  }

  // ---------------------------------------------------------------- the entries that carry a live share

  /** The rows a share of these parts stands in, and on which page: a thing's own page, and for a place with what is in it also its defaults page. */
  private placesOfRows(kind: string, inside: boolean): { defaults: boolean; areas: readonly Area[] }[] {
    if (kind === 'thread') return [{ defaults: false, areas: AREAS }];
    return [
      { defaults: false, areas: ['chat', 'documents'] },
      ...(inside ? [{ defaults: true, areas: AREAS }] : []),
    ];
  }

  private rowKeys(page: StoredPage, permission: string): string[] {
    return [...page.rows.keys()].filter((key) => key.startsWith(`${permission}|`));
  }

  /** Names the agents of a place in the rows of the areas, at the levels the parts say; the entries made are returned. */
  private write(person: Person, thing: string, kind: string, parts: ShareParts, inside: boolean, to: string, expires: string | null): Carried[] {
    const carried: Carried[] = [];
    for (const target of this.placesOfRows(kind, inside)) {
      const page = this.access.pageFor(thing, target.defaults);
      for (const area of target.areas) {
        const level = parts[area];
        if (level === undefined || level === null) continue;
        const names: readonly string[] = (ROWS[area] as Record<string, readonly string[]>)[level] ?? [];
        for (const permission of names) {
          for (const key of this.rowKeys(page, permission)) {
            const row = page.rows.get(key);
            const profile = key.split('|')[1] ?? 'any';
            if (row === undefined) continue;
            const known = row.entries.find((entry) => entry.holder.kind === 'agents' && entry.holder.node.id === to);
            if (known !== undefined) {
              if (expires !== null) known.expires_at = expires;
              carried.push({ id: thing, defaults: target.defaults, permission, profile, entryId: known.id });
              continue;
            }
            this.access.apply(person, page, { rows: [{ permission, ...(profile === 'any' ? {} : { profile: profile as 'managed' | 'connected' }), add: [{ holder: { kind: 'agents', id: to }, ...(expires === null ? {} : { expires_at: expires }) }] }] });
            const made = row.entries.find((entry) => entry.holder.kind === 'agents' && entry.holder.node.id === to);
            if (made !== undefined) carried.push({ id: thing, defaults: target.defaults, permission, profile, entryId: made.id });
          }
        }
      }
    }
    return carried;
  }

  private withdraw(share: Share): void {
    for (const carried of share.carried) {
      const page = this.access.pageFor(carried.id, carried.defaults);
      const row = page.rows.get(`${carried.permission}|${carried.profile}`);
      if (row !== undefined) row.entries = row.entries.filter((entry) => entry.id !== carried.entryId);
      page.revision += 1;
    }
    share.carried = [];
  }

  // ---------------------------------------------------------------- policy

  private reachAt(id: string): 'none' | 'topic' | 'project' {
    for (const place of [id, ...this.access.ancestorsOf(id)]) {
      const set = this.reach.get(place);
      if (set !== undefined) return set;
    }
    return 'none';
  }

  /** The setting `sharing.agent_reach` as the settings of a node list it: its value, and the place that set it. */
  settingOf(id: string) {
    const from = [id, ...this.access.ancestorsOf(id)].find((place) => this.reach.has(place));
    const source = from === undefined ? { scope: 'default' as const, node: null } : { scope: (this.nodeRef(from).kind) as 'org' | 'project' | 'topic', node: this.nodeRef(from) };
    return { key: 'sharing.agent_reach', value: this.reachAt(id), source, own: null, locked_by: null, writable: true };
  }

  /** The setting `sharing.snapshot_max_bytes` as the settings of a node list it: the most a copy may come to, which whoever may change it can raise from the form. */
  limitSetting() {
    return { key: 'sharing.snapshot_max_bytes', value: this.snapshotLimit, source: { scope: 'default' as const, node: null }, own: null, locked_by: null, writable: true };
  }

  // ---------------------------------------------------------------- the requests

  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;
    if (route === 'POST /share/agents') return this.create(person, body);
    if ((found = /^GET \/(nodes|documents|chats)\/([^/]+)\/agent-shares$/.exec(route))) return this.list(person, found[1] ?? 'nodes', found[2] ?? '', query.get('side') ?? 'of');
    if ((found = /^(PATCH|DELETE) \/share\/agents\/([^/]+)$/.exec(route))) return found[1] === 'DELETE' ? this.stop(person, found[2] ?? '') : this.change(person, found[2] ?? '', body);
    if ((found = /^GET \/threads\/([^/]+)\/shared$/.exec(route))) return this.shared(person, found[1] ?? '');
    if (/^GET \/threads\/[^/]+\/shared\/tree$/.test(route)) return { status: 200, body: { path: query.get('path') ?? '/', entries: [], truncated: false } };
    return null;
  }

  private visible(person: Person, share: Share): boolean {
    return share.what.kind !== 'node' || this.access.holdingOn(person, share.what.id).held.has(`${this.nodeRef(share.what.id).kind}.view`) || this.access.holdingOn(person, share.what.id).held.has('chat.view');
  }

  private list(person: Person, object: string, id: string, side: string): Answer {
    if (object === 'nodes') this.access.holdingOn(person, id);
    const scope = side === 'to' ? [id, ...this.access.ancestorsOf(id)] : [];
    const found = this.records.filter((share) => (side === 'to' ? scope.includes(share.to) : share.what.id === id) && this.visible(person, share));
    return { status: 200, body: found.map((share) => this.shape(share)) };
  }

  private shared(person: Person, threadId: string): Answer {
    const scope = [threadId, ...this.access.ancestorsOf(threadId)];
    const mine = this.records.filter((share) => scope.includes(share.to) && this.visible(person, share));
    const topic = this.access.ancestorsOf(threadId)[0] ?? threadId;
    return { status: 200, body: { policy: this.reachAt(topic), shares: mine.map((share) => this.shape(share)) } };
  }

  /** Whether a person may share what the parts lets the agents do, and may share it at all: what the page of the thing says. */
  private checkSharer(person: Person, thing: string, parts: ShareParts): void {
    const here = this.access.holdingOn(person, thing);
    const kind = this.nodeRef(thing).kind;
    // A place has no conversation rows on its own page: what its threads give is judged by what the reader holds of the place, as a role there would.
    const place = kind === 'org' || kind === 'project' || kind === 'topic';
    const asked = needed(parts).map((name) => (place && name.startsWith('thread.') ? name.replace('thread.', `${kind}.`) : name));
    const missing = asked.filter((name) => !here.held.has(name));
    if (missing.length > 0) throw this.access.lackingOn(thing, missing);
    if (!here.mayShare) throw this.access.lackingOn(thing, ['access.manage']);
  }

  /** Runs the same in-memory producer on the access world's trial pages, retaining neither entries nor copies. */
  preview(person: Person, path: string, id: string, body: ShareWithAgents): SharePreview {
    const source = 'node_id' in body.what ? body.what.node_id : 'document_id' in body.what ? body.what.document_id : body.what.chat_id;
    const object = 'node_id' in body.what ? 'nodes' : 'document_id' in body.what ? 'documents' : 'chats';
    if (id !== source || path !== object) throw new Refusal(400, 'request.invalid', 'the share source must match its preview');
    const prior = this.records;
    this.records = [...prior];
    try {
      const kind = object === 'nodes' ? this.nodeRef(id).kind : object === 'documents' ? 'document' : 'chat';
      const requested: ShareParts = body.parts ?? (object === 'nodes' ? { timeline:'read' } : {});
      const revision = object === 'chats' ? null : this.access.pageFor(id,false,path).revision;
      const inside = body.inside !== false && ['org','project','topic'].includes(kind);
      const destinations = body.to.map((to) => 'thread_id' in to ? to.thread_id : to.node_id);
      const beforeRows: SharePreviewRow[] = object !== 'nodes' ? [] : this.placesOfRows(kind,inside).flatMap((target) => {
        const page = this.access.pageFor(id,target.defaults);
        return [...page.rows].map(([key,row]): SharePreviewRow => {
          const [permission = '', profile] = key.split('|');
          return { defaults:target.defaults, permission, profile:profile === 'managed' || profile === 'connected' ? profile : 'any', before:row.entries.filter((entry) => entry.holder.kind === 'agents' && destinations.includes(entry.holder.node.id)), after:[] };
        });
      });
      this.create(person,{...body});
      const made = this.records.filter((share) => share.clientId === body.client_id);
      const rows = beforeRows.map((row) => ({...row,after:this.access.pageFor(id,row.defaults).rows.get(`${row.permission}|${row.profile}`)?.entries.filter((entry) => entry.holder.kind === 'agents' && destinations.includes(entry.holder.node.id)) ?? []})).filter((row) => row.before.length > 0 || row.after.length > 0);
      const rank = {read:1,write:2,edit:2,manage:3};
      return {
        allowed:true,revision,
        permissions:needed(requested),holders:[],effects:{pages:object === 'nodes' ? this.placesOfRows(kind,inside).length : 1,orgs:null,gaining:[],losing:[],more:0},already:[],already_more:0,rows,
        agents:made.map((share) => {
          const already: ShareParts = {};
          for (const old of prior.filter((old) => old.mode === 'live' && old.what.id === id && old.to === share.to && old.inside === share.inside)) for (const area of AREAS) {
            const level = old.parts[area], current = already[area];
            if (level !== undefined && level !== null && (current === undefined || current === null || rank[level] > rank[current])) Object.assign(already,{[area]:level});
          }
          return {to:this.nodeRef(share.to),inside:share.inside,parts:share.parts,already_shared:already,download:share.download,future_threads:this.nodeRef(share.to).kind !== 'thread',copy:share.snapshot === null ? null : {files:share.snapshot.files,bytes:share.snapshot.bytes,already_sent:prior.some((old) => old.clientId === body.client_id)}};
        }),
        rules:[{key:'sharing.agent_reach',value:this.reachAt(id),source:null,sharing:true},{key:'sharing.snapshot_max_bytes',value:this.snapshotLimit,source:null,sharing:true}],refusal:null,
      };
    } finally {this.records=prior;}
  }

  private create(person: Person, body: Record<string, unknown>): Answer {
    const what = is(body.what) ? body.what : {};
    const mode = body.mode === 'snapshot' ? 'snapshot' : 'live';
    const to = Array.isArray(body.to) ? (body.to as Record<string, unknown>[]) : [];
    const clientId = typeof body.client_id === 'string' ? body.client_id : randomUUID();
    const done = this.records.filter((share) => share.clientId === clientId);
    if (done.length > 0) return { status: 201, body: { shares: done.map((share) => this.shape(share)) } };
    if (to.length === 0 || to.length > DESTINATIONS_MAX) throw new Refusal(400, 'request.invalid', 'name between one and ten threads or places', { errors: [{ path: 'to', message: 'name between one and ten' }] });
    const whatKind = typeof what.node_id === 'string' ? 'node' : typeof what.chat_id === 'string' ? 'chat' : 'document';
    const thing = [what.node_id, what.chat_id, what.document_id].find((value): value is string => typeof value === 'string') ?? '';
    const kind = whatKind === 'node' ? this.nodeRef(thing).kind : whatKind;
    const placeAlone = body.inside === false && ['org', 'project', 'topic'].includes(kind);
    const parts: ShareParts = is(body.parts) ? body.parts : whatKind === 'node' ? { timeline: 'read' } : {};
    if (placeAlone && parts.timeline !== undefined && parts.timeline !== null) throw new Refusal(400, 'request.invalid', 'a place has no conversation of its own', { errors: [{ path: 'parts.timeline', message: 'a place shared by itself has no conversation' }] });
    if (mode === 'snapshot' && Object.values(parts).includes('write')) throw new Refusal(400, 'request.invalid', 'a copy holds no write', { errors: [{ path: 'parts', message: 'a copy holds no write' }] });

    if (whatKind === 'node') this.access.holdingOn(person, thing);
    const destinations = to.map((entry) => [entry.thread_id, entry.node_id].find((value): value is string => typeof value === 'string') ?? '');
    for (const destination of destinations) {
      const ref = this.nodeRef(destination);
      const holding = this.access.holdingOn(person, destination);
      const permission = `${ref.kind}.interact`;
      if (!holding.held.has(permission)) throw this.access.lackingOn(destination, [permission]);
      const topic = ref.kind === 'thread' ? (this.access.ancestorsOf(destination)[0] ?? destination) : destination;
      if (this.toolsOff.has(topic)) throw new Refusal(409, 'share.unavailable', 'the topic of that thread does not enable the tools of the shared tree', { reason: 'tools_off', thread: ref });
    }
    if (whatKind === 'node') this.checkSharer(person, thing, parts);

    if (mode === 'snapshot') {
      if (!this.copiesKept) throw new Refusal(501, 'capability.unsupported', 'a copy cannot be kept yet', { capability: 'share.copy' });
      const bytes = Math.max(1, areasShared(parts)) * this.areaBytes * destinations.length;
      if (bytes > this.snapshotLimit) throw new Refusal(413, 'share.too_large', 'the copy is larger than sharing.snapshot_max_bytes allows where it lies', { bytes, limit: this.snapshotLimit, scope: this.nodeRef(this.access.ancestorsOf(thing)[0] ?? thing) });
    }

    const expires = typeof body.expires_at === 'string' ? body.expires_at : null;
    const made: Share[] = destinations.map((destination) => {
      const share: Share = {
        id: randomUUID(),
        what: { kind: whatKind, id: thing },
        to: destination,
        mode,
        inside: placeAlone ? false : whatKind === 'node' && ['org', 'project', 'topic'].includes(kind),
        download: body.download !== false,
        parts,
        expires_at: expires,
        by: person,
        at: this.access.now(),
        carried: [],
        snapshot: null,
        clientId,
      };
      if (mode === 'live' && whatKind === 'node') share.carried = this.write(person, thing, kind, parts, share.inside, destination, expires);
      if (mode === 'snapshot') {
        const bytes = Math.max(1, areasShared(parts)) * this.areaBytes;
        share.snapshot = { path: `.orch/shared/${share.id.slice(0, 8)}`, files: 3, bytes, as_of: share.at, state: 'delivered' };
      }
      return share;
    });
    this.records.push(...made);
    return { status: 201, body: { shares: made.map((share) => this.shape(share)) } };
  }

  /** Whoever shared it, or whoever could share the same thing now. */
  private mayChange(person: Person, share: Share): void {
    if (share.by.me.id === person.me.id) return;
    if (share.what.kind === 'node') this.checkSharer(person, share.what.id, share.parts);
  }

  private change(person: Person, id: string, body: Record<string, unknown>): Answer {
    const share = this.records.find((entry) => entry.id === id);
    if (share === undefined) throw new Refusal(404, 'not_found', 'no such share');
    this.mayChange(person, share);
    if (share.mode !== 'live') throw new Refusal(400, 'request.invalid', 'a copy cannot be changed');
    const parts = is(body.parts) ? (body.parts as ShareParts) : share.parts;
    if (is(body.parts) && share.what.kind === 'node') this.checkSharer(person, share.what.id, parts);
    const expires = 'expires_at' in body ? (typeof body.expires_at === 'string' ? body.expires_at : null) : share.expires_at;
    this.withdraw(share);
    share.parts = parts;
    share.expires_at = expires;
    if (share.what.kind === 'node') share.carried = this.write(person, share.what.id, this.nodeRef(share.what.id).kind, parts, share.inside, share.to, expires);
    return { status: 200, body: this.shape(share) };
  }

  private stop(person: Person, id: string): Answer {
    const share = this.records.find((entry) => entry.id === id);
    if (share === undefined) throw new Refusal(404, 'not_found', 'no such share');
    this.mayChange(person, share);
    this.withdraw(share);
    this.records = this.records.filter((entry) => entry !== share);
    return { status: 204 };
  }

  // ---------------------------------------------------------------- the requests only the double has

  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/reach':
        this.reach.set(String(body.place), body.value === 'topic' || body.value === 'project' ? body.value : 'none');
        return { status: 204 };
      case '/tools-off':
        if (body.off === false) this.toolsOff.delete(String(body.topic));
        else this.toolsOff.add(String(body.topic));
        return { status: 204 };
      case '/snapshot':
        if (typeof body.limit === 'number') this.snapshotLimit = body.limit;
        if (typeof body.area === 'number') this.areaBytes = body.area;
        if (typeof body.kept === 'boolean') this.copiesKept = body.kept;
        return { status: 204 };
      case '/list':
        return { status: 200, body: this.records.map((share) => this.shape(share)) };
      default:
        return null;
    }
  }
}
