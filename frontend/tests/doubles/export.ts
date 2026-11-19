/** The fake API of exports: a thread, a chat and a document as one file, the plan of an archive of a place and the job that writes it, the archives of each person with their removal and their download, and the threads of a topic.
 *
 * It keeps to what the real API promises as far as a screen can tell: the same members, the same codes
 * (`export.too_large`, `export.too_many`, `export.nothing`, `429` past two at once, `410` for a removed archive)
 * and the same rule of who may take what away: a thread, a chat and a document by their Export rows, which as shipped
 * whoever reads them holds, a direct or group chat by being in it. A job advances by itself, or is held and finished by
 * a test; it can fail with a reason. Everything is in memory and is forgotten when the world is reset. The requests
 * only the double has, which steer it, are under `/__double/export`.
 */
import { randomUUID } from 'node:crypto';
import { crc32, deflateRawSync } from 'node:zlib';

import type {
  ExportCounts,
  ExportLeftOut,
  ExportLimits,
  ExportPlan,
  ExportRequest,
  ExportState,
  Job,
  NodeRef,
  Thread,
  TreeNode,
} from '../../src/generated/api-types.ts';
import { ACME, userRef } from './fixtures.ts';
import type { Doc } from './documents.ts';
import { type Answer, type Part, type Person, Refusal, type World } from './world.ts';

const KIB = 1024;
const GIB = 1024 * 1024 * 1024;

/** What a place allows of one archive unless a test says otherwise: the shipped defaults. */
const SHIPPED_LIMITS: ExportLimits = { max_bytes: 2 * GIB, max_items: 10_000, keep_hours: 24 };

/** How many archives of one person may be written at the same time. */
const AT_ONCE = 2;

/** What a thing weighs in an archive, so that a plan has sizes to speak of. */
const CONVERSATION_BYTES = 24 * KIB;
const CHAT_BYTES = 8 * KIB;

/** How long a job takes unless a test holds it. */
const JOB_MS = 1_200;

/** How long a removed or ended archive stays in the list, in days. */
const PAGE = 20;

/** One archive of a person. */
interface Archive {
  id: string;
  user: string;
  node: NodeRef | null;
  request: ExportRequest;
  state: ExportState;
  jobId: string;
  bytes: number | null;
  counts: ExportCounts | null;
  leftOut: ExportLeftOut[];
  createdAt: number;
  readyAt: number | null;
  expiresAt: number | null;
  downloadedAt: number | null;
  /** What it was written to hold, for a test to read back. */
  lines: string[];
  /** The job was given up on by its person. */
  stopped: boolean;
}

/** The job that writes an archive. */
interface Making {
  id: string;
  archive: string;
  user: string;
  startedAt: number;
  /** Held jobs are finished by a test; the others by the clock. */
  held: boolean;
  percent: number;
  /** When set, the job ends in this failure. */
  failure: { code: string; message: string } | null;
  endedAs: 'succeeded' | 'failed' | 'cancelled' | null;
  endedAt: number | null;
}

/** What one reading of a place came to for one person. */
interface Reading {
  conversations: { id: string; title: string; bytes: number; home: string }[];
  chats: { id: string; title: string; bytes: number; home: string }[];
  documents: { doc: Doc; bytes: number }[];
  leftOut: Map<string, number>;
  files: number;
  places: number;
}

const iso = (at: number): string => new Date(at).toISOString();

/** How a job is to end badly, as a test says it: a code and the words, each with a plain default; nothing when it says none. */
function failureOf(value: unknown): { code: string; message: string } | null {
  if (typeof value !== 'object' || value === null) return null;
  const given = value as { code?: unknown; message?: unknown };
  return { code: typeof given.code === 'string' ? given.code : 'internal', message: typeof given.message === 'string' ? given.message : 'the archive could not be written' };
}

const refuse = (status: number, code: string, detail: string, extra: Record<string, unknown> = {}): never => {
  throw new Refusal(status, code as never, detail, extra);
};

/** A zip with one stored file: a real archive, as small as one can be. */
export function zipOf(name: string, text: string): Uint8Array {
  return zipFilesOf([[name, Buffer.from(text, 'utf8')]]);
}

/** Real stored/deflated entries for exports that carry their image bytes too. */
export function zipFilesOf(entries: Iterable<[string, Uint8Array]>, deflated = false): Uint8Array {
  const locals: Buffer[] = [];
  const centrals: Buffer[] = [];
  let offset = 0;
  let count = 0;
  for (const [name, bytes] of entries) {
  const file = Buffer.from(name, 'utf8');
  const data = Buffer.from(bytes);
  const compressed = deflated ? deflateRawSync(data) : data;
  const crc = crc32(data);
  const local = Buffer.alloc(30);
  local.writeUInt32LE(0x04034b50, 0);
  local.writeUInt16LE(20, 4);
  local.writeUInt16LE(0x0800, 6);
  local.writeUInt16LE(deflated ? 8 : 0, 8);
  local.writeUInt32LE(crc, 14);
  local.writeUInt32LE(compressed.length, 18);
  local.writeUInt32LE(data.length, 22);
  local.writeUInt16LE(file.length, 26);
  const central = Buffer.alloc(46);
  central.writeUInt32LE(0x02014b50, 0);
  central.writeUInt16LE(20, 4);
  central.writeUInt16LE(20, 6);
  central.writeUInt16LE(0x0800, 8);
  central.writeUInt16LE(deflated ? 8 : 0, 10);
  central.writeUInt32LE(crc, 16);
  central.writeUInt32LE(compressed.length, 20);
  central.writeUInt32LE(data.length, 24);
  central.writeUInt16LE(file.length, 28);
  central.writeUInt32LE(offset, 42);
  locals.push(local, file, compressed);
  centrals.push(central, file);
  offset += local.length + file.length + compressed.length;
  count++;
  }
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(count, 8);
  end.writeUInt16LE(count, 10);
  end.writeUInt32LE(centrals.reduce((sum, part) => sum + part.length, 0), 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, ...centrals, end]);
}

/** A header value that carries any name, as the real API writes it. */
function disposition(name: string): string {
  const plain = name.replace(/[^\x20-\x7e]|["\\]/g, '_');
  return `attachment; filename="${plain}"; filename*=UTF-8''${encodeURIComponent(name).replace(/'/g, '%27')}`;
}

export class ExportWorld implements Part {
  readonly world: World;
  private archives = new Map<string, Archive>();
  private jobs = new Map<string, Making>();
  /** What a place allows, by place; the shipped limits for the others. */
  limits = new Map<string, ExportLimits>();
  /** Whether jobs are held for a test to finish them. */
  hold = false;
  /** How long a job takes when it is not held. */
  jobMs = JOB_MS;
  /** The failure the next job ends in, when a test said so. */
  nextFailure: { code: string; message: string } | null = null;
  /** The documents some people may read and not take away, as `username:document`. */
  barredDocuments = new Set<string>();
  /** What the single exports were asked with, for a test to read. */
  asked: { path: string; query: Record<string, string>; person: string }[] = [];
  /** How long every answer of this part is held back. */
  delayMs = 0;

  constructor(world: World) {
    this.world = world;
  }

  // ---------------------------------------------------------------- who may take what away

  private tree(placeId: string): { root: string; nodes: Map<string, TreeNode> } {
    const root = this.world.places.rootOf(placeId);
    const nodes = root === null ? undefined : this.world.trees.get(root)?.nodes;
    if (root === null || nodes === undefined) return refuse(404, 'not_found', 'no such place');
    return { root, nodes };
  }

  /** The place and everything under it that has not been deleted. */
  private within(placeId: string): TreeNode[] {
    const { nodes } = this.tree(placeId);
    const found: TreeNode[] = [];
    const at = nodes.get(placeId);
    if (at === undefined || at.deleted) return refuse(404, 'not_found', 'no such place');
    const visit = (row: TreeNode) => {
      found.push(row);
      for (const child of nodes.values()) if (child.parent_id === row.id && !child.deleted) visit(child);
    };
    visit(at);
    return found;
  }

  private holds(person: Person, nodeId: string, permission: string): boolean {
    return this.world.places.access(person, nodeId).permissions.includes(permission);
  }

  private mayReadNode(person: Person, nodeId: string): boolean {
    return this.world.places.access(person, nodeId).permissions.includes('node.view') || this.world.places.access(person, nodeId).permissions.some((one) => one.endsWith('.view'));
  }

  /** Whether somebody who may read a document may not take it away: a test said so. */
  barred(person: Person, documentId: string): boolean {
    return this.barredDocuments.has(`${person.me.username}:${documentId}`);
  }

  private canTakeDocument(person: Person, doc: Doc): boolean {
    return this.holds(person, doc.nodeId, 'doc.export') && !this.barred(person, doc.id);
  }

  private limitsFor(placeId: string): ExportLimits {
    for (const row of this.world.places.pathTo(placeId).reverse()) {
      const set = this.limits.get(row.id);
      if (set !== undefined) return set;
    }
    return this.limits.get('*') ?? SHIPPED_LIMITS;
  }

  // ---------------------------------------------------------------- reading a place

  private read(person: Person, placeId: string, request: ExportRequest): Reading {
    this.world.places.node(person, placeId);
    const rows = this.within(placeId);
    const inScope = new Set(rows.map((row) => row.id));
    const chosen = request.what === 'all' ? null : request.what;
    const included = new Set<string>();
    if (chosen === null) for (const row of rows) included.add(row.id);
    else {
      const takeWhole = (id: string) => {
        if (!inScope.has(id)) return;
        included.add(id);
        for (const row of rows) if (row.parent_id === id) takeWhole(row.id);
      };
      for (const id of chosen.nodes ?? []) takeWhole(id);
    }
    const parts = request.parts ?? {};
    const detail = request.detail ?? 'normal';
    const reading: Reading = { conversations: [], chats: [], documents: [], leftOut: new Map(), files: 0, places: 0 };
    const leave = (home: string) => reading.leftOut.set(home, (reading.leftOut.get(home) ?? 0) + 1);

    reading.places = rows.filter((row) => included.has(row.id) && row.kind !== 'thread').length;
    if (parts.conversations !== false) {
      for (const row of rows) {
        if (row.kind !== 'thread' || !included.has(row.id) || !this.mayReadNode(person, row.id)) continue;
        if (!this.holds(person, row.id, 'thread.export')) {
          leave(row.parent_id ?? row.id);
          continue;
        }
        const scale = detail === 'brief' ? 0.2 : detail === 'full' ? 3 : 1;
        reading.conversations.push({ id: row.id, title: row.name, bytes: Math.round(CONVERSATION_BYTES * scale), home: row.parent_id ?? row.id });
      }
    }
    if (parts.chats !== false) {
      const wanted = new Set<string>(chosen?.chats ?? []);
      for (const row of rows) {
        const has = (this.world.chats.records.get(row.chat.id)?.lastMessageSeq ?? row.chat.seq) > 0;
        if (!has || !(included.has(row.id) || wanted.has(row.chat.id))) continue;
        if (!this.holds(person, row.id, 'chat.export')) {
          leave(row.id);
          continue;
        }
        reading.chats.push({ id: row.chat.id, title: row.name, bytes: CHAT_BYTES, home: row.id });
      }
    }
    if (parts.documents !== false) {
      const wanted = new Set<string>(chosen?.documents ?? []);
      const documents = this.world.documents;
      for (const doc of documents.readable(person)) {
        if (!inScope.has(doc.nodeId) || !(included.has(doc.nodeId) || wanted.has(doc.id))) continue;
        if (!this.canTakeDocument(person, doc)) {
          leave(doc.nodeId);
          continue;
        }
        const ref = documents.ref(person, doc);
        const files = request.files === true ? doc.files.reduce((sum, file) => sum + file.size, 0) : 0;
        const earlier = request.versions === true ? ref.bytes * (ref.version - 1) : 0;
        reading.documents.push({ doc, bytes: ref.bytes + files + earlier });
        if (request.files === true) reading.files += doc.files.length;
      }
    }
    return reading;
  }

  private plan(person: Person, placeId: string, request: ExportRequest): ExportPlan {
    const reading = this.read(person, placeId, request);
    const limits = this.limitsFor(placeId);
    const json = request.format === 'json' ? 1.5 : 1;
    const bytes = Math.round((reading.conversations.reduce((sum, one) => sum + one.bytes, 0) + reading.chats.reduce((sum, one) => sum + one.bytes, 0) + reading.documents.reduce((sum, one) => sum + one.bytes, 0)) * json);
    const items = reading.conversations.length + reading.chats.length + reading.documents.length;
    const counts: ExportCounts = { places: reading.places, conversations: reading.conversations.length, chats: reading.chats.length, documents: reading.documents.length, files: reading.files };
    const blockers: ExportPlan['blockers'] = [];
    if (items === 0) blockers.push({ code: 'export.nothing', message: 'there is nothing in it that you may take away as a file' });
    else {
      if (items > limits.max_items) blockers.push({ code: 'export.too_many', message: `it would hold ${items} things, and the place allows ${limits.max_items}` });
      if (bytes > limits.max_bytes) blockers.push({ code: 'export.too_large', message: `it would come to about ${bytes} bytes, and the place allows ${limits.max_bytes}` });
    }
    const leftOut: ExportLeftOut[] = [...reading.leftOut].map(([home, count]) => ({ what: 'not_exportable' as const, count, where: this.refOf(home) }));
    return { node: this.refOf(placeId), counts, bytes, limits, left_out: leftOut, blockers };
  }

  private refOf(nodeId: string): NodeRef {
    const row = this.world.places.rowOf(nodeId);
    return { id: nodeId, kind: row?.kind ?? 'org', name: row?.name ?? null, redacted: false };
  }

  // ---------------------------------------------------------------- the archives

  private move(): void {
    const now = Date.now();
    for (const job of this.jobs.values()) {
      const archive = this.archives.get(job.archive);
      if (archive === undefined || job.endedAs !== null) continue;
      if (!job.held) job.percent = Math.min(99, Math.floor(((now - job.startedAt) / this.jobMs) * 100));
      if (job.held || now - job.startedAt < this.jobMs) continue;
      this.end(job, job.failure === null ? 'succeeded' : 'failed');
    }
    for (const archive of this.archives.values()) {
      if (archive.state === 'ready' && archive.expiresAt !== null && archive.expiresAt <= now) archive.state = 'expired';
    }
  }

  private end(job: Making, how: 'succeeded' | 'failed' | 'cancelled'): void {
    const archive = this.archives.get(job.archive);
    const now = Date.now();
    job.endedAs = how;
    job.endedAt = now;
    job.percent = how === 'succeeded' ? 100 : job.percent;
    if (archive === undefined) return;
    if (how === 'succeeded') {
      archive.state = 'ready';
      archive.readyAt = now;
      archive.expiresAt = now + this.limitsFor(archive.node?.id ?? '').keep_hours * 3_600_000;
    } else {
      archive.state = how === 'failed' ? 'failed' : 'expired';
      if (how === 'cancelled') archive.expiresAt = now;
    }
  }

  private jobView(job: Making, person: Person): Job {
    const archive = this.archives.get(job.archive);
    const state = job.endedAs ?? (Date.now() - job.startedAt < 30 ? 'queued' : 'running');
    const phase = job.percent < 5 ? 'select' : job.percent < 95 ? 'write' : 'store';
    const total = archive?.counts === undefined || archive.counts === null ? 40 : archive.counts.conversations + archive.counts.chats + archive.counts.documents;
    return {
      batch_id: null,
      created_at: iso(job.startedAt),
      created_by: userRef(person.me),
      environment_id: null,
      error: job.endedAs === 'failed' && job.failure !== null ? { ...job.failure, phase: 'write', undone: true } : null,
      finished_at: job.endedAt === null ? null : iso(job.endedAt),
      id: job.id,
      kind: 'export',
      log: null,
      node_id: archive?.node?.id ?? null,
      progress: {
        bytes_done: Math.round((job.percent / 100) * 4_000_000),
        bytes_total: 4_000_000,
        eta_seconds: null,
        files_done: 0,
        files_total: total,
        message: `${Math.round((job.percent / 100) * total)} of ${total} things`,
        percent: job.percent,
        phase,
        phases: [
          { name: 'select', state: job.percent >= 5 ? 'done' : 'running', weight: 5 },
          { name: 'write', state: job.percent >= 95 ? 'done' : job.percent >= 5 ? 'running' : 'pending', weight: 90 },
          { name: 'store', state: job.endedAs === 'succeeded' ? 'done' : 'pending', weight: 5 },
        ],
        rate_bps: null,
      },
      provider_id: null,
      result: job.endedAs === 'succeeded' ? { export_id: job.archive } : null,
      started_at: iso(job.startedAt),
      state,
    };
  }

  private view(archive: Archive) {
    return {
      id: archive.id,
      node: archive.node,
      request: archive.request,
      state: archive.state,
      job_id: archive.jobId,
      bytes: archive.bytes,
      counts: archive.counts,
      left_out: archive.leftOut,
      created_at: iso(archive.createdAt),
      ready_at: archive.readyAt === null ? null : iso(archive.readyAt),
      expires_at: archive.expiresAt === null ? null : iso(archive.expiresAt),
      downloaded_at: archive.downloadedAt === null ? null : iso(archive.downloadedAt),
    };
  }

  private mine(person: Person, id: string): Archive {
    const found = this.archives.get(id);
    if (found?.user !== person.me.username) return refuse(404, 'not_found', 'no such archive');
    return found;
  }

  private create(person: Person, placeId: string, request: ExportRequest): Answer {
    const plan = this.plan(person, placeId, request);
    const [first] = plan.blockers;
    if (first !== undefined) {
      const status = 409;
      const items = plan.counts.conversations + plan.counts.chats + plan.counts.documents;
      if (first.code === 'export.nothing') return refuse(status, 'export.nothing', first.message);
      if (first.code === 'export.too_many') return refuse(status, 'export.too_many', first.message, { limit: plan.limits.max_items, asked: items });
      return refuse(status, 'export.too_large', first.message, { limit: plan.limits.max_bytes, asked: plan.bytes });
    }
    this.move();
    const writing = [...this.archives.values()].filter((one) => one.user === person.me.username && one.state === 'pending').length;
    if (writing >= AT_ONCE) {
      return refuse(429, 'rate_limited', 'as many archives of yours are being written as may be at once; ask again when one is ready', { limit: AT_ONCE });
    }
    const now = Date.now();
    const archive: Archive = {
      id: randomUUID(),
      user: person.me.username,
      node: this.refOf(placeId),
      request,
      state: 'pending',
      jobId: randomUUID(),
      bytes: null,
      counts: null,
      leftOut: [],
      createdAt: now,
      readyAt: null,
      expiresAt: null,
      downloadedAt: null,
      lines: [],
      stopped: false,
    };
    const job: Making = {
      id: archive.jobId,
      archive: archive.id,
      user: person.me.username,
      startedAt: now,
      held: this.hold,
      percent: 0,
      failure: this.nextFailure,
      endedAs: null,
      endedAt: null,
    };
    this.nextFailure = null;
    // What it will hold is decided now, as the plan said: the double does not read the place again when the job ends.
    archive.bytes = plan.bytes;
    archive.counts = plan.counts;
    archive.leftOut = plan.left_out;
    archive.lines = this.linesOf(person, placeId, request);
    this.archives.set(archive.id, archive);
    this.jobs.set(job.id, job);
    return { status: 202, body: this.jobView(job, person) };
  }

  private linesOf(person: Person, placeId: string, request: ExportRequest): string[] {
    const reading = this.read(person, placeId, request);
    return [
      `# ${this.refOf(placeId).name ?? 'place'}`,
      ...reading.conversations.map((one) => `conversation: ${one.title}`),
      ...reading.chats.map((one) => `chat: ${one.title}`),
      ...reading.documents.map((one) => `document: ${one.doc.title}`),
    ];
  }

  private list(person: Person, query: URLSearchParams): Answer {
    this.move();
    const mine = [...this.archives.values()].filter((one) => one.user === person.me.username).sort((one, other) => other.createdAt - one.createdAt);
    const offset = Math.max(0, Number(query.get('cursor') ?? 0) || 0);
    const limit = Math.min(200, Math.max(1, Number(query.get('limit') ?? PAGE) || PAGE));
    const items = mine.slice(offset, offset + limit).map((one) => this.view(one));
    return { status: 200, body: { items, next_cursor: offset + limit < mine.length ? String(offset + limit) : null } };
  }

  private remove(person: Person, id: string): Answer {
    this.move();
    const archive = this.mine(person, id);
    if (archive.state === 'expired') return refuse(404, 'not_found', 'no such archive');
    const job = this.jobs.get(archive.jobId);
    if (job?.endedAs === null) this.end(job, 'cancelled');
    archive.state = 'expired';
    archive.expiresAt ??= Date.now();
    return { status: 204 };
  }

  private download(person: Person, id: string): Answer {
    this.move();
    const archive = this.mine(person, id);
    if (archive.state === 'pending') return refuse(404, 'not_found', 'no such archive');
    if (archive.state !== 'ready') return refuse(410, 'gone', 'the archive was removed');
    archive.downloadedAt ??= Date.now();
    const day = iso(Date.now()).slice(0, 10);
    const name = `${archive.node?.name ?? 'export'}-${day}.zip`;
    const bytes = zipOf('index.md', `${archive.lines.join('\n')}\n`);
    return { status: 200, bytes, headers: { 'Content-Type': 'application/zip', 'Content-Disposition': disposition(name), 'Content-Length': String(bytes.length) } };
  }

  // ---------------------------------------------------------------- one thing as one file

  private note(person: Person, path: string, query: URLSearchParams): void {
    this.asked.push({ path, query: Object.fromEntries(query), person: person.me.username });
  }

  private file(name: string, type: string, text: string): Answer {
    const bytes = Buffer.from(text, 'utf8');
    return { status: 200, bytes, headers: { 'Content-Type': type, 'Content-Disposition': disposition(name), 'Content-Length': String(bytes.length) } };
  }

  private thread(person: Person, id: string, query: URLSearchParams): Answer {
    const row = this.world.places.rowOf(id);
    if (row?.kind !== 'thread') return refuse(404, 'not_found', 'no such thread');
    this.world.places.need(person, id, 'thread.view');
    this.world.places.need(person, id, 'thread.export');
    const include = (query.get('include') ?? 'sub_streams').split(',').filter((word) => word !== '');
    if (include.includes('chat')) {
      this.world.places.need(person, id, 'chat.view');
      this.world.places.need(person, id, 'chat.export');
    }
    this.note(person, `/threads/${id}/export`, query);
    const json = query.get('format') === 'json';
    const text = json
      ? JSON.stringify({ thread: row.name, detail: query.get('detail'), from: query.get('from'), to: query.get('to'), include }, null, 2)
      : `# ${row.name}\n\ndetail=${query.get('detail') ?? 'normal'} include=${include.join(',')} from=${query.get('from') ?? ''} to=${query.get('to') ?? ''}\n`;
    return this.file(`${row.name}.${json ? 'json' : 'md'}`, json ? 'application/json' : 'text/markdown', text);
  }

  private chat(person: Person, id: string, query: URLSearchParams): Answer {
    const { records } = this.world.chats;
    const found = records.get(id);
    const nodeRow = found === undefined ? [...this.world.trees.values()].flatMap((tree) => [...tree.nodes.values()]).find((row) => row.chat.id === id) : undefined;
    if (found === undefined && nodeRow === undefined) return refuse(404, 'not_found', 'no such chat');
    if (found !== undefined && found.kind !== 'node') {
      if (!found.members.includes(person.me.username)) return refuse(403, 'authz.denied', 'only the people in the chat take it away', { required: 'chat.export', requestable: false, override: false, missing: [] });
    } else {
      this.world.places.need(person, nodeRow?.id ?? found?.nodeId ?? '', 'chat.export');
    }
    this.note(person, `/chats/${id}/export`, query);
    const json = query.get('format') === 'json';
    const title = found?.name ?? nodeRow?.name ?? 'chat';
    const text = json ? JSON.stringify({ chat: title, from: query.get('from'), to: query.get('to') }, null, 2) : `# Chat of ${title}\n\nfiles=${query.get('files') ?? 'omit'} from=${query.get('from') ?? ''} to=${query.get('to') ?? ''}\n`;
    if (query.get('files') === 'include') {
      const bytes = zipOf(`chat.${json ? 'json' : 'md'}`, text);
      return { status: 200, bytes, headers: { 'Content-Type': 'application/zip', 'Content-Disposition': disposition(`${title}.zip`), 'Content-Length': String(bytes.length) } };
    }
    return this.file(`${title}.${json ? 'json' : 'md'}`, json ? 'application/json' : 'text/markdown', text);
  }

  /** A person of the world as a reference; the first administrator when the name is not one the world knows. */
  private personRef(username: string) {
    const person = this.world.people.get(username) ?? this.world.people.get('ada');
    if (person === undefined) throw new Error('the world has nobody to name');
    return userRef(person.me);
  }

  private documentFile(person: Person, id: string, query: URLSearchParams): Answer {
    const documents = this.world.documents;
    const doc = documents.docs.get(id);
    const may = doc === undefined || doc.deleted ? null : documents.readable(person).find((one) => one.id === id);
    if (doc === undefined || may === null || may === undefined) return refuse(404, 'not_found', 'no such document');
    if (!this.canTakeDocument(person, doc)) {
      const owner = this.personRef(doc.owner);
      return refuse(403, 'authz.denied', 'doc.export is required on this document', {
        required: 'doc.export',
        object: { kind: 'document', id },
        requestable: true,
        override: false,
        missing: [{ object: { kind: 'document', id, name: doc.title, owner }, permissions: ['doc.export'], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
      });
    }
    this.note(person, `/documents/${id}/export`, query);
    const format = query.get('format') ?? 'markdown';
    const withFiles = query.get('attachments') === 'include' && doc.files.length > 0;
    const said = documents.rendered(person, doc, format, query.get('comments') === 'include');
    const losses: Record<string, string> = said.losses.length > 0 ? { 'X-Orch-Export-Losses': said.losses.join(', ') } : {};
    if (withFiles || format === 'typst') {
      const bytes = zipOf(format === 'typst' ? 'main.typ' : `${doc.title}.${said.extension}`, said.text);
      return { status: 200, bytes, headers: { 'Content-Type': 'application/zip', 'Content-Disposition': disposition(`${doc.title}.zip`), 'Content-Length': String(bytes.length), ...losses } };
    }
    const answer = this.file(`${doc.title}.${said.extension}`, said.type, said.text);
    return { ...answer, headers: { ...answer.headers, ...losses } };
  }

  private topicSync(person: Person, topicId: string): Answer {
    const topic = this.world.places.node(person, topicId);
    if (topic.kind !== 'topic') return refuse(404, 'not_found', 'no such topic');
    const threads: Thread[] = [];
    for (const row of this.within(topicId)) {
      if (row.kind === 'thread') {
        const view = this.world.places.node(person, row.id);
        if (view.kind === 'thread') threads.push(view);
      }
    }
    return { status: 200, body: { announcements: [], complete: true, jobs: [], seq: 1, since: null, threads, topic } };
  }

  // ---------------------------------------------------------------- the part

  async handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Promise<Answer | null> {
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;
    const mine = /^(GET|POST|DELETE) \/(threads|chats|documents|exports|jobs|me\/exports|nodes|topics)(\/|$)/.test(route);
    if (!mine) return null;
    if (this.delayMs > 0) await new Promise((resolve) => setTimeout(resolve, this.delayMs));
    if ((found = /^POST \/nodes\/([^/]+)\/exports\/preflight$/.exec(route))) return { status: 200, body: this.plan(person, found[1] ?? '', body as unknown as ExportRequest) };
    if ((found = /^POST \/nodes\/([^/]+)\/exports$/.exec(route))) return this.create(person, found[1] ?? '', body as unknown as ExportRequest);
    if (route === 'GET /me/exports') return this.list(person, query);
    if ((found = /^GET \/exports\/([^/]+)$/.exec(route))) {
      this.move();
      return { status: 200, body: this.view(this.mine(person, found[1] ?? '')) };
    }
    if ((found = /^GET \/exports\/([^/]+)\/download$/.exec(route))) return this.download(person, found[1] ?? '');
    if ((found = /^DELETE \/exports\/([^/]+)$/.exec(route))) return this.remove(person, found[1] ?? '');
    if ((found = /^GET \/jobs\/([^/]+)$/.exec(route))) {
      this.move();
      const job = this.jobs.get(found[1] ?? '');
      if (job?.user !== person.me.username) return null;
      return { status: 200, body: this.jobView(job, person) };
    }
    if ((found = /^POST \/jobs\/([^/]+)\/cancel$/.exec(route))) {
      const job = this.jobs.get(found[1] ?? '');
      if (job?.user !== person.me.username) return null;
      if (job.endedAs === null) this.end(job, 'cancelled');
      return { status: 200, body: this.jobView(job, person) };
    }
    if ((found = /^GET \/threads\/([^/]+)\/export$/.exec(route))) return this.thread(person, found[1] ?? '', query);
    if ((found = /^GET \/chats\/([^/]+)\/export$/.exec(route))) return this.chat(person, found[1] ?? '', query);
    if ((found = /^GET \/documents\/([^/]+)\/export$/.exec(route))) return this.documentFile(person, found[1] ?? '', query);
    if ((found = /^GET \/topics\/([^/]+)\/sync$/.exec(route))) return this.topicSync(person, found[1] ?? '');
    return null;
  }

  /** What only a test can do. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    switch (path) {
      case '/limits': {
        // What a place allows: of one place, or `*` for all of them.
        const place = typeof body.place === 'string' ? body.place : '*';
        const held = this.limits.get(place) ?? SHIPPED_LIMITS;
        this.limits.set(place, {
          max_bytes: typeof body.max_bytes === 'number' ? body.max_bytes : held.max_bytes,
          max_items: typeof body.max_items === 'number' ? body.max_items : held.max_items,
          keep_hours: typeof body.keep_hours === 'number' ? body.keep_hours : held.keep_hours,
        });
        return { status: 204 };
      }
      case '/jobs': {
        // Whether jobs are held for `finish`, how long the others take, and how the next one ends.
        if (typeof body.hold === 'boolean') this.hold = body.hold;
        if (typeof body.ms === 'number') this.jobMs = body.ms;
        if (body.fail !== undefined) this.nextFailure = failureOf(body.fail);
        return { status: 204 };
      }
      case '/progress': {
        for (const job of this.jobs.values()) if (job.endedAs === null && (body.job === undefined || body.job === job.id)) job.percent = Math.max(0, Math.min(99, Number(body.percent) || 0));
        return { status: 204 };
      }
      case '/finish': {
        // Ends a held job: the newest running one, or the one named; as it was bound to end unless a test says it fails.
        const running = [...this.jobs.values()].filter((job) => job.endedAs === null && (body.job === undefined || body.job === job.id));
        const job = running.at(-1);
        if (job === undefined) return { status: 404, body: { detail: 'no job is running' } };
        const failure = failureOf(body.fail);
        if (failure !== null) job.failure = failure;
        this.end(job, job.failure === null ? 'succeeded' : 'failed');
        return { status: 200, body: { archive: job.archive } };
      }
      case '/bar': {
        // Takes the right to export a thing from somebody: a node (a thread, or the chat and the documents of a place), or one document.
        const username = typeof body.username === 'string' ? body.username : '';
        const permission = typeof body.permission === 'string' ? body.permission : '';
        const object = typeof body.object === 'string' ? body.object : '';
        if (this.world.documents.docs.has(object)) this.barredDocuments.add(`${username}:${object}`);
        else {
          const held = this.world.places.denied.get(username) ?? new Set<string>();
          held.add(`${permission}@${object}`);
          this.world.places.denied.set(username, held);
        }
        return { status: 204 };
      }
      case '/asked':
        return { status: 200, body: this.asked };
      case '/made':
        return { status: 200, body: [...this.archives.values()].map((one) => ({ ...this.view(one), user: one.user, lines: one.lines })) };
      case '/age': {
        // Moves the end of every ready archive: one that has run out of time is expired by the next look.
        const by = typeof body.ms === 'number' ? body.ms : 0;
        for (const one of this.archives.values()) if (one.expiresAt !== null) one.expiresAt -= by;
        return { status: 204 };
      }
      case '/purge-place': {
        // The place of an archive is gone: purged for good.
        const place = typeof body.place === 'string' ? body.place : '';
        for (const one of this.archives.values()) if (one.node?.id === place) one.node = null;
        return { status: 204 };
      }
      case '/delay':
        this.delayMs = typeof body.ms === 'number' ? body.ms : 0;
        return { status: 204 };
      case '/seed-archive': {
        // An archive that was made earlier, in the state a test needs: removed, expired, failed, or ready.
        const user = typeof body.username === 'string' ? body.username : 'ada';
        const state = (typeof body.state === 'string' ? body.state : 'ready') as ExportState;
        const place = typeof body.place === 'string' ? body.place : ACME.web;
        const now = Date.now();
        const back = typeof body.ago_ms === 'number' ? body.ago_ms : 3_600_000;
        // One that is still being written has no size, no counts and no end yet, as one that failed has none.
        const unfinished = state === 'failed' || state === 'pending';
        const archive: Archive = {
          id: randomUUID(),
          user,
          node: this.refOf(place),
          request: typeof body.request === 'object' && body.request !== null ? (body.request as ExportRequest) : { what: 'all' },
          state,
          jobId: randomUUID(),
          bytes: unfinished ? null : 123_456,
          counts: unfinished ? null : { places: 3, conversations: 4, chats: 1, documents: 5, files: typeof body.files === 'number' ? body.files : 0 },
          // What the server left out of what was asked for: as the export's view carries it, once the archive is ready.
          leftOut: !unfinished && Array.isArray(body.left_out) ? (body.left_out as Archive['leftOut']) : [],
          createdAt: now - back,
          readyAt: unfinished || body.cancelled === true ? null : now - back + 60_000,
          expiresAt: unfinished ? null : typeof body.expires_in_ms === 'number' ? now + body.expires_in_ms : now - back + 24 * 3_600_000,
          downloadedAt: null,
          lines: [`# ${this.refOf(place).name ?? 'place'}`],
          stopped: false,
        };
        const job: Making = {
          id: archive.jobId,
          archive: archive.id,
          user,
          startedAt: archive.createdAt,
          held: false,
          percent: state === 'ready' || state === 'expired' ? 100 : 40,
          failure: state === 'failed' ? { code: 'export.too_large', message: 'the archive passed the bytes the place allows' } : null,
          endedAs: state === 'failed' ? 'failed' : state === 'pending' ? null : 'succeeded',
          endedAt: archive.createdAt + 60_000,
        };
        if (state === 'pending') job.held = true;
        this.archives.set(archive.id, archive);
        this.jobs.set(job.id, job);
        return { status: 200, body: { id: archive.id, job: job.id } };
      }
      default:
        return null;
    }
  }
}
