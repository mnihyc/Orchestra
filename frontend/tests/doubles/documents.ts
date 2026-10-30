/** What the fake API says of documents: pages of blocks beside a node, the batches that change them with the block as it is now when a change came second, versions, discussions, exports, and the feed of a document with who has it open and who is writing where.
 *
 * It keeps to what the real API promises of these requests, as far as a screen can tell: the same
 * shapes, the same refusals, a conflict that names the blocks as they stand, a batch that is applied
 * whole or not at all and answers a repeat as it answered the first time. Everything is in memory.
 * A test plays the other people with the requests under `/__double/documents`.
 */
import { randomUUID } from 'node:crypto';

import { readMarkdown, type Piece } from './markdown-reader.ts';
import { readHtml } from '../../src/features/people/html.ts';

import type {
  Author,
  Document,
  DocumentBlock,
  DocumentChanges,
  DocumentComment,
  DocumentFile,
  DocumentDiscussion,
  DocumentOp,
  DocumentRef,
  DocumentVersion,
  DocumentTreeListed,
  DocumentTreeOperationBody,
  DocumentTreeRead,
  DocumentTreePage,
  DocumentTreeReceipt,
  Inline,
  NodeRef,
  RelatedItems,
  DocumentLinks,
  GraphNode,
  LinkGraph,
  LinkedThing,
  LinkOccurrence,
  LinksTab,
  TableRow,
  TreeNode,
  UserRef,
} from '../../src/generated/api-types.ts';
import { ACME, MADE_AT, id as fixtureId, userRef } from './fixtures.ts';
import { type Answer, type Part, type Person, Refusal, type Subscriber, type World } from './world.ts';

/** The most blocks a document of the fake holds, and the bytes of their text. */
const DEFAULT_LIMITS = { bytes: 2 * 1024 * 1024, blocks: 5_000, file_bytes: 8 * 1024 ** 3, files_bytes: 1024 ** 3 };
/** The most an operation batch may hold, as the real API takes. */
const OPS_MAX = 500;
/** The compact list's shipped count; weighted ranking itself is verified against the real API. */
const RELATED_ITEMS_DEFAULT = 8;
/** The tabs of the Links section, in the order it shows them. */
const LINK_TABS: LinksTab[] = ['referenced_by', 'references', 'related', 'graph'];
const BLOCK_TYPES = new Set([
  'paragraph', 'heading', 'bullet', 'numbered', 'todo', 'toggle', 'quote', 'callout', 'code', 'math', 'diagram', 'divider',
  'page_break', 'image', 'video', 'audio', 'file', 'table', 'columns', 'column',
]);
const LEAF_TYPES = new Set(['code', 'math', 'diagram', 'divider', 'page_break', 'image', 'video', 'audio', 'file', 'table']);

interface Span {
  authors: Map<string, Author>;
  inserted: number;
  updated: number;
  moved: number;
  removed: number;
  from: string;
  to: string;
}

interface Version {
  version: number;
  title: string;
  blocks: DocumentBlock[];
  cause: DocumentVersion['cause'];
  authors: Author[];
  changed: DocumentVersion['changed'];
  from: string;
  to: string;
}

export interface Doc {
  id: string;
  nodeId: string;
  title: string;
  /** What it is about, and who keeps that: a person or an agent that wrote it, or automation. */
  description: string;
  descriptionSource: 'user' | 'auto';
  owner: string;
  createdBy: Author;
  seq: number;
  blocks: Map<string, DocumentBlock>;
  discussions: Map<string, DocumentDiscussion>;
  versions: Version[];
  span: Span | null;
  page: 'follows' | 'restricted';
  deleted: boolean;
  createdAt: string;
  updatedAt: string;
  updatedBy: Author | null;
  /** What each person last said they had on screen, and the version that stood then. */
  seen: Map<string, { seq: number; version: number }>;
  /** Replies to batches, by author and client identifier, so that a repeat answers as the first did. */
  done: Map<string, unknown>;
  limits: Document['limits'];
  archivedOverride: boolean | null;
  /** The files attached to it. */
  files: DocumentFile[];
  /** What a document seeded for an archive is said to weigh, to have been through and to have open, where its blocks do not say it. */
  said: { bytes?: number; version?: number; open?: number };
}

interface Watcher {
  subscriber: Subscriber;
  subId: number;
  doc: string;
  person: Person;
  lastSent: number;
  /** When a typing sign was last passed on, so that no more than one in a few seconds goes. */
  signedAt: number;
}

/** The least time between two signs of writing from one person that are passed on. */
const SIGN_EVERY_MS = 3_000;

const asText = (value: unknown): string => (typeof value === 'string' ? value : '');
const now = (): string => new Date().toISOString();
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;
/** The words a block holds, as a reader of its text sees them. */
const wordsOf = (block: DocumentBlock): string =>
  Array.isArray(block.content) ? (block.content as Inline[]).map((run) => (run.type === 'text' ? run.text : '')).join('') : '';

export class Documents implements Part {
  readonly world: World;
  docs = new Map<string, Doc>();
  private relatedAnswers = new Map<string, Answer>();
  /** What each document's owner chose to show of the Links section; none follows the place. */
  private linkTabs = new Map<string, LinksTab[]>();
  /** Documents whose tabs a lock above fixes at the place's setting. */
  private linkLocks = new Set<string>();
  private watchers: Watcher[] = [];
  private counter = 0;
  /** Frames to withhold from live connections, to look at what a gap does. */
  private lose = false;

  constructor(world: World) {
    this.world = world;
  }

  // ---------------------------------------------------------------- who holds what

  private person(username: string): Person | undefined {
    return this.world.people.get(username);
  }

  private userOf(username: string): UserRef {
    const person = this.person(username);
    if (person === undefined) return { id: fixtureId(1, 0), kind: 'human', username, display_name: username, avatar: { text: username.slice(0, 1).toUpperCase(), color: 0 } };
    return userRef(person.me);
  }

  private node(nodeId: string): TreeNode {
    const path = this.world.places.pathTo(nodeId);
    const found = path[path.length - 1];
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such place');
    return found;
  }

  private nodeRef(nodeId: string): NodeRef {
    const found = this.node(nodeId);
    return { id: found.id, kind: found.kind, name: found.name, redacted: false };
  }

  // ------------------------------------------------------------------ the Links section

  private linksOf(person: Person, doc: Doc): DocumentLinks {
    const own = this.linkTabs.get(doc.id) ?? null;
    const locked = this.linkLocks.has(doc.id);
    return { shown: locked ? [...LINK_TABS] : own ?? [...LINK_TABS], own, inherited: [...LINK_TABS], locked, writable: !locked && this.holdings(person, doc).permissions.includes('doc.manage') };
  }

  /** Where `source` mentions `target`: each block with a chip for it, and the words round the chip. */
  private mentionsOf(source: Doc, target: Doc): LinkOccurrence[] {
    const found: LinkOccurrence[] = [];
    for (const block of source.blocks.values()) {
      const runs: Inline[] = Array.isArray(block.content) ? (block.content as Inline[]) : [];
      const at = runs.findIndex((run) => run.type === 'ref' && run.kind === 'document' && run.id === target.id);
      if (at < 0) continue;
      const words = (parts: Inline[]) => parts.map((run) => (run.type === 'text' ? run.text : '')).join('');
      found.push({ anchor: { type: 'block', block_id: block.id }, line: { before: words(runs.slice(0, at)), reference: target.title, after: words(runs.slice(at + 1)) }, by: null, at: block.updated_at });
    }
    return found;
  }

  private readableDocs(person: Person): Doc[] {
    return [...this.docs.values()].filter((doc) => !doc.deleted && this.holdings(person, doc).permissions.includes('doc.view'));
  }

  private placeOf(doc: Doc) { return this.path(doc.nodeId).map((node) => ({ id: node.id, name: node.name })); }

  private referencesOf(person: Person, doc: Doc, query: URLSearchParams): { items: LinkedThing[]; next_cursor: string | null } {
    const limit = Number(query.get('limit') ?? 20);
    const offset = Number(query.get('cursor') ?? 0) || 0;
    const incoming = query.get('direction') === 'in';
    const things: LinkedThing[] = incoming
      ? this.readableDocs(person).filter((other) => other.id !== doc.id).map((other): [Doc, LinkOccurrence[]] => [other, this.mentionsOf(other, doc)]).filter(([, mentions]) => mentions.length > 0)
        .sort((left, right) => right[0].id.localeCompare(left[0].id))
        .map(([other, mentions]): LinkedThing => ({ kind: 'document', id: other.id, name: other.title, label: null, readable: true, context: this.placeOf(other), mentions: mentions.length, mentions_more: false, occurrences: mentions.slice(0, 5), next_mentions: null }))
      : [...new Map([...doc.blocks.values()].flatMap((block) => (Array.isArray(block.content) ? (block.content as Inline[]) : []).filter((run) => run.type === 'ref' && run.kind === 'document' && run.id !== doc.id).map((run) => [run.type === 'ref' ? run.id : '', run] as const))).values()].map((run): LinkedThing => {
        const id = run.type === 'ref' ? run.id : '';
        const target = this.docs.get(id);
        const readable = target !== undefined && !target.deleted && this.holdings(person, target).permissions.includes('doc.view');
        return { kind: 'document', id, name: readable ? target.title : null, label: run.type === 'ref' ? run.label ?? null : null, readable, context: readable ? this.placeOf(target) : [], mentions: 1, mentions_more: false, occurrences: [], next_mentions: null };
      });
    const page = things.slice(offset, offset + limit);
    return { items: page, next_cursor: offset + limit < things.length ? String(offset + limit) : null };
  }

  private graphOf(person: Person, doc: Doc, query: URLSearchParams): LinkGraph {
    const depth = Math.min(3, Math.max(1, Number(query.get('depth') ?? 1) || 1));
    const readable = this.readableDocs(person);
    const edges = readable.flatMap((from) => readable.filter((to) => to.id !== from.id && this.mentionsOf(from, to).length > 0).map((to) => ({ from: from.id, to: to.id, kind: 'reference' as const })));
    const nodes = new Map<string, GraphNode>([[doc.id, { kind: 'document', id: doc.id, name: doc.title, context: this.placeOf(doc), distance: 0, current: true }]]);
    let frontier = [doc.id];
    for (let step = 1; step <= depth; step += 1) {
      const next: string[] = [];
      for (const edge of edges) {
        for (const [near, far] of [[edge.from, edge.to], [edge.to, edge.from]] as const) {
          if (!frontier.includes(near) || nodes.has(far)) continue;
          const other = this.docs.get(far);
          if (other === undefined) continue;
          nodes.set(far, { kind: 'document', id: far, name: other.title, context: this.placeOf(other), distance: step, current: false });
          next.push(far);
        }
      }
      frontier = next;
    }
    return { center: doc.id, nodes: [...nodes.values()], edges: edges.filter((edge) => nodes.has(edge.from) && nodes.has(edge.to)), truncated: false, meaning: 'off' };
  }

  private path(nodeId: string): NodeRef[] {
    return this.world.places.pathTo(nodeId).map((row) => ({ id: row.id, kind: row.kind, name: row.name, redacted: false }));
  }

  private isArchived(doc: Doc): boolean {
    return doc.archivedOverride ?? this.node(doc.nodeId).archived;
  }

  /** What the person holds on a document: its node's rows, unless it was made private. */
  private holdings(person: Person, doc: Doc): { permissions: string[]; administers: boolean; owns: boolean } {
    const owns = doc.owner === person.me.username;
    const role = this.world.places.roleAt(person, doc.nodeId);
    const system = person.me.is_system_admin;
    const administers = role === 'admin' || system;
    const page = this.world.access.storedHoldingOn(person, doc.id, 'documents');
    if (page !== null) return { permissions: [...page].sort(), administers, owns };
    const takes = this.world.places.access(person, doc.nodeId).permissions.includes('doc.export') && !this.world.exports.barred(person, doc.id) ? ['doc.export'] : [];
    if (doc.page === 'restricted') {
      return owns || system
        ? { permissions: ['doc.view', 'doc.comment', 'doc.edit', 'doc.manage', 'access.manage', 'access.own', ...takes].sort(), administers, owns }
        : { permissions: [], administers, owns };
    }
    const rank = system ? 3 : role === null ? -1 : ['viewer', 'interactor', 'developer', 'admin'].indexOf(role);
    if (rank < 0 && !owns) return { permissions: [], administers, owns };
    const permissions = ['doc.view', 'doc.comment'];
    if (rank >= 1 || owns) permissions.push('doc.edit');
    if (rank >= 2 || owns) permissions.push('doc.manage');
    if (rank >= 3 || owns) permissions.push('access.manage', 'access.own');
    return { permissions: [...permissions, ...takes].sort(), administers, owns };
  }

  /** How many documents are written beside a node, as far as a person may see them: those that follow the node, and those made private (which only whoever holds them counts). The permission page of a place tells these. */
  countsBeside(person: Person, nodeId: string): { open: number; closed: number } {
    const counts = { open: 0, closed: 0 };
    for (const doc of this.docs.values()) {
      if (doc.deleted || doc.nodeId !== nodeId || !this.holdings(person, doc).permissions.includes('doc.view')) continue;
      if (doc.page === 'restricted') counts.closed += 1;
      else counts.open += 1;
    }
    return counts;
  }

  private deny(person: Person, doc: Doc, permission: string): never {
    const hold = this.holdings(person, doc);
    // A reader who cannot see the document is told nothing of it.
    if (!hold.permissions.includes('doc.view')) throw new Refusal(404, 'not_found', 'no such document');
    const owner = this.userOf(doc.owner);
    const object = { kind: 'document', id: doc.id };
    throw new Refusal(403, 'authz.denied', `${permission} is required on this document`, {
      required: permission,
      object,
      requestable: true,
      override: false,
      missing: [{ object: { ...object, name: doc.title, owner }, permissions: [permission], options: null, ask: [owner], ask_more: 0, override: false, closed: null }],
    });
  }

  private need(person: Person, doc: Doc, permission: string, writes = false): void {
    if (!this.holdings(person, doc).permissions.includes(permission)) this.deny(person, doc, permission);
    if (writes && this.isArchived(doc)) throw new Refusal(409, 'node.archived', 'the place is archived: the document is read-only');
  }

  private find(person: Person, id: string, deleted = false): Doc {
    const doc = this.docs.get(id);
    if (doc === undefined || (doc.deleted && !deleted)) throw new Refusal(404, 'not_found', 'no such document');
    if (!this.holdings(person, doc).permissions.includes('doc.view')) throw new Refusal(404, 'not_found', 'no such document');
    return doc;
  }

  private authorOf(person: Person): Author {
    return userRef(person.me);
  }

  // ---------------------------------------------------------------- shapes

  private bytesOf(doc: Doc): number {
    let total = 0;
    for (const block of doc.blocks.values()) if (!block.removed) total += JSON.stringify(block.props).length + JSON.stringify(block.content).length;
    return total;
  }

  private live(doc: Doc): DocumentBlock[] {
    return [...doc.blocks.values()].filter((block) => !block.removed);
  }

  /** The real reader resolves stored mention identifiers to the current visible person. */
  private expandMentions<T>(value: T): T {
    const expand = (part: unknown): unknown => {
      if (Array.isArray(part)) return part.map(expand);
      if (part === null || typeof part !== 'object') return part;
      const object = part as Record<string, unknown>;
      const shown = Object.fromEntries(Object.entries(object).map(([key, child]) => [key, expand(child)]));
      if (object.type === 'mention' && typeof object.user_id === 'string') {
        const person = [...this.world.people.values()].find((one) => one.me.id === object.user_id);
        shown.user = person === undefined ? null : userRef(person.me);
      }
      return shown;
    };
    return expand(value) as T;
  }

  /** What a list shows beside the title, as the server words it: the description's first line, or else the first paragraph's. */
  private lineOf(doc: Doc): string {
    const first = (text: string) => text.split('\n').map((line) => line.trim()).find((line) => line !== '') ?? '';
    const own = first(doc.description);
    if (own !== '') return own;
    const paragraph = this.live(doc)
      .filter((block) => block.parent_id === null && block.type === 'paragraph' && wordsOf(block).trim() !== '')
      .sort((a, b) => (a.position < b.position ? -1 : a.position > b.position ? 1 : a.id < b.id ? -1 : 1))[0];
    return paragraph === undefined ? '' : first(wordsOf(paragraph));
  }

  ref(person: Person, doc: Doc): DocumentRef {
    const seen = doc.seen.get(person.me.username);
    return {
      id: doc.id,
      title: doc.title,
      description: doc.description,
      description_source: doc.descriptionSource,
      description_line: this.lineOf(doc),
      node: this.nodeRef(doc.nodeId),
      owner: this.userOf(doc.owner),
      owners: 1,
      created_by: doc.createdBy,
      page: doc.page === 'restricted' ? 'restricted' : 'follows',
      seq: doc.seq,
      block_count: this.live(doc).length,
      bytes: doc.said.bytes ?? this.bytesOf(doc),
      files_bytes: doc.files.reduce((sum, file) => sum + file.size, 0),
      version: doc.said.version ?? doc.versions.length,
      open_discussions: doc.said.open ?? [...doc.discussions.values()].filter((discussion) => discussion.resolved === null).length,
      updated_at: doc.updatedAt,
      updated_by: doc.updatedBy,
      discussion_level: 'involved',
      seen_seq: seen?.seq ?? null,
      archived: this.isArchived(doc),
      deleted: doc.deleted,
    };
  }

  /** Current read and administrative-place facts, without copying content or changing a page. */
  openingAccess(person: Person, id: string): { id: string; name: string; place: 'system' | null } | null {
    const doc = this.docs.get(id);
    if (doc === undefined || doc.deleted) throw new Refusal(404, 'not_found', 'no such document');
    if (!this.holdings(person, doc).permissions.includes('doc.view')) return null;
    const ordinary = this.holdings({ ...person, me: { ...person.me, is_system_admin: false } }, doc);
    return { id: doc.id, name: doc.title, place: ordinary.permissions.includes('doc.view') ? null : 'system' };
  }

  full(person: Person, doc: Doc): Document {
    const held = this.holdings(person, doc);
    const name = doc.title.trim().replace(/[/\\\p{Control}]/gu, '-').trim();
    const basename = (['', '.', '..'].includes(name) ? '_' + name + '_' : name) + '.md';
    // Use the same page resolver without native system reach, rather than treating an administrator as a participant.
    const ordinary = person.me.is_system_admin ? this.holdings({ ...person, me: { ...person.me, is_system_admin: false } }, doc) : held;
    const participating = ordinary.permissions.includes('doc.view');
    return {
      ...this.ref(person, doc),
      path: this.path(doc.nodeId),
      location: { parent: { kind: 'node_docs', id: doc.nodeId }, basename, ancestors: [] },
      files: clone(doc.files),
      limits: doc.limits,
      created_at: doc.createdAt,
      access: {
        ...held,
        participating,
        admin_only: held.permissions.filter((permission) => !ordinary.permissions.includes(permission)),
        admin_access: !participating && held.permissions.includes('doc.view') ? { place: 'system', private: doc.page === 'restricted' } : null,
      },
    };
  }

  /** The documents the person may read, for the finder and the search of what was written: each one's reference, the way to it, and the words of each block that has any. */
  forSearch(person: Person): { ref: DocumentRef; path: NodeRef[]; blocks: { id: string; text: string; by: Author | null; at: string }[] }[] {
    return [...this.docs.values()]
      .filter((doc) => !doc.deleted && this.holdings(person, doc).permissions.includes('doc.view'))
      .map((doc) => ({
        ref: this.ref(person, doc),
        path: this.path(doc.nodeId),
        blocks: this.live(doc).flatMap((block) => (wordsOf(block) === '' ? [] : [{ id: block.id, text: wordsOf(block), by: block.updated_by, at: block.updated_at }])),
      }));
  }

  /** The documents written beside a node that the person may read, the most recently changed first. */
  list(person: Person, nodeId: string, query: URLSearchParams) {
    this.world.places.need(person, nodeId, `${this.node(nodeId).kind}.view`);
    const under = query.get('under') === 'true';
    const nodes = new Set([nodeId]);
    if (under) for (const doc of this.docs.values()) if (this.world.places.pathTo(doc.nodeId).some((row) => row.id === nodeId)) nodes.add(doc.nodeId);
    const onlyDeleted = query.get('deleted') === 'only';
    const rows = [...this.docs.values()]
      .filter((doc) => nodes.has(doc.nodeId) && doc.deleted === onlyDeleted && this.holdings(person, doc).permissions.includes('doc.view'))
      .sort((one, other) => (one.updatedAt < other.updatedAt ? 1 : one.updatedAt > other.updatedAt ? -1 : one.id < other.id ? 1 : -1));
    const limit = Math.min(200, Number(query.get('limit') ?? 50));
    const start = Number(query.get('cursor') ?? 0);
    const page = rows.slice(start, start + limit);
    return { items: page.map((doc) => this.ref(person, doc)), next_cursor: start + limit < rows.length ? String(start + limit) : null };
  }

  /** Metadata-only root-document views for the same browser fixture objects, without a parallel editor model. */
  private treeRow(person: Person, doc: Doc): DocumentTreeListed {
    const value = this.full(person, doc), root = value.path[0];
    if (root === undefined) throw new Refusal(404, 'not_found', 'the document has no place');
    return { entry: { item: { kind: 'document', id: doc.id }, parent: value.location.parent,
      node: doc.nodeId, root: root.id, basename: value.location.basename, title: doc.title,
      owner: value.owner.id, revision: doc.seq, deleted_at: doc.deleted ? doc.updatedAt : null,
      deletion: null, files: doc.files.length, versions: doc.versions.length },
      document: doc.deleted ? null : this.ref(person, doc), standing: { read: value.access.permissions.includes('doc.view'),
        write: value.access.permissions.includes('doc.edit'), manage: value.access.permissions.includes('doc.manage') } };
  }

  private treeHandle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const item = /^\/document-tree\/entries\/([^/]+)$/.exec(path);
    if (method === 'GET' && item !== null && query.get('kind') === 'document') {
      const doc = this.find(person, item[1] ?? '', query.get('inspect_deleted') === 'true');
      if (doc.deleted) this.need(person, doc, 'doc.manage');
      return { status: 200, body: { entries: [this.treeRow(person, doc)], ancestors: [], place: { node: this.nodeRef(doc.nodeId), path: this.path(doc.nodeId) } } satisfies DocumentTreeRead };
    }
    const directory = /^\/nodes\/([^/]+)\/document-tree$/.exec(path);
    if (method === 'GET' && directory !== null && query.get('parent_container_id') === null) {
      const node = this.world.places.node(person, directory[1] ?? '');
      const listed = this.list(person, node.id, query);
      const page: DocumentTreePage = { items: listed.items.map((row) => this.treeRow(person, this.find(person, row.id, row.deleted))),
        ancestors: [], next_cursor: listed.next_cursor, feed: `fixture-root-documents:${node.id}`,
        directory: { at: { kind: 'node_docs', id: node.id }, node: this.nodeRef(node.id), path: node.path, folder: null,
          ancestors: [], can_create: !node.archived && node.access.permissions.includes('doc.create'), inspection: null, membership_seq: node.revision } };
      return { status: 200, body: page };
    }
    if (method !== 'POST' || path !== '/document-tree/operations' && path !== '/document-tree/delete/preview') return null;
    const request = body as unknown as DocumentTreeOperationBody;
    if (request.items.length !== 1 || request.items[0]?.kind !== 'document') throw new Refusal(400, 'request.invalid', 'this fixture operation requires one text document');
    const doc = this.find(person, request.items[0].id, request.command === 'restore');
    this.need(person, doc, request.command === 'copy' ? 'doc.view' : request.command === 'rename' ? 'doc.edit' : 'doc.manage');
    const row = this.treeRow(person, doc);
    // The fixed promise is scenario data, as in the retained-content browser fixture.
    const until = '2026-11-11T12:00:00.000Z';
    if (path.endsWith('/preview')) return { status: 200, body: { gaining: 0, losing: 0, restore_until: until,
      stamp: { entries: [row.entry], authority_epoch: 1, authority_fingerprint: String(doc.seq), authority_until: null } } };
    let current = doc;
    if (request.command === 'delete') {
      if (request.deletion_plan?.entries[0]?.revision !== doc.seq) throw new Refusal(409, 'conflict.stale_revision', 'read the changed deletion preview again');
      const snapshot = this.sync(person, doc, null);
      this.handle(person, 'DELETE', `/documents/${doc.id}`, query, {}); doc.updatedAt = now();
      this.world.trace.retained.set(doc.id, { kind: 'document', name: doc.title, home: doc.nodeId, state: 'deleted', document: snapshot,
        inspection: { deleted_at: doc.updatedAt, deleted_by: userRef(person.me), purge_at: until, readonly: true } });
    } else if (request.command === 'restore') this.handle(person, 'POST', `/documents/${doc.id}/restore`, query, {});
    else if (request.command === 'rename') this.handle(person, 'PATCH', `/documents/${doc.id}`, query, { title: request.name });
    else if (request.command === 'move' || request.command === 'copy') {
      if (request.to?.kind !== 'node_docs') throw new Refusal(400, 'request.invalid', 'select an existing document directory');
      if (request.command === 'move') {
        const before = doc.seq; this.handle(person, 'POST', `/documents/${doc.id}/move`, query, { node_id: request.to.id });
        this.broadcast(doc, [], [], before);
      }
      else {
        const answer = this.world.peopleWork.handle(person, 'POST', `/nodes/${request.to.id}/documents/copy`, query,
          { source_document_id: doc.id, access: 'follows', client_id: request.client_id });
        const copied = answer?.body as Document | undefined;
        if (copied === undefined || typeof copied.id !== 'string') throw new Refusal(400, 'request.invalid', 'the copy could not be made');
        current = this.find(person, copied.id);
      }
      if (request.name !== null) this.handle(person, 'PATCH', `/documents/${current.id}`, query, { title: request.name.replace(/\.md$/, '') });
    } else throw new Refusal(400, 'request.invalid', 'the operation does not apply to text');
    const changed = this.treeRow(person, current);
    const receipt: DocumentTreeReceipt = { operation: randomUUID(), entries: [changed.entry], view: { entries: [changed], ancestors: [], place: { node: this.nodeRef(current.nodeId), path: this.path(current.nodeId) } },
      copied: request.command === 'copy' ? [{ source: row.entry.item, destination: changed.entry.item }] : [],
      gaining: 0, losing: 0, restore_until: request.command === 'delete' ? until : null };
    return { status: 200, body: receipt };
  }

  // ---------------------------------------------------------------- making and reading

  private blockRow(doc: Doc, id: string, parent: string | null, position: string, type: DocumentBlock['type'], props: unknown, content: unknown, author: Author, seq: number): DocumentBlock {
    return {
      id,
      document_id: doc.id,
      parent_id: parent,
      position,
      type,
      props: (props ?? {}),
      content: (content ?? []) as DocumentBlock['content'],
      removed: false,
      seq_created: seq,
      seq_updated: seq,
      updated_by: author,
      updated_at: now(),
    };
  }

  private make(
    person: Person,
    nodeId: string,
    title: string,
    blocks: unknown[],
    access: 'follows' | 'private',
    extra: { id?: string; files?: { name: string; size: number; media_type: string }[]; said?: Doc['said'] } = {},
  ): Doc {
    const author = this.authorOf(person);
    this.counter += 1;
    const doc: Doc = {
      id: extra.id ?? fixtureId(7, this.counter),
      nodeId,
      title,
      description: '',
      descriptionSource: 'auto',
      owner: person.me.username,
      createdBy: author,
      seq: 1,
      blocks: new Map(),
      discussions: new Map(),
      versions: [],
      span: null,
      page: access === 'private' ? 'restricted' : 'follows',
      deleted: false,
      createdAt: now(),
      updatedAt: now(),
      updatedBy: author,
      seen: new Map(),
      done: new Map(),
      limits: { ...DEFAULT_LIMITS },
      archivedOverride: null,
      files: (extra.files ?? []).map((file, at) => ({
        added_at: MADE_AT,
        added_by: null,
        id: fixtureId(0x32, this.counter * 100 + at),
        is_text: file.media_type.startsWith('text/'),
        media_type: file.media_type,
        name: file.name,
        size: file.size,
      })),
      said: extra.said ?? {},
    };
    let at = 0;
    const place = (items: unknown[], parent: string | null) => {
      for (const item of items) {
        const block = item as { id?: string; type: DocumentBlock['type']; props?: unknown; content?: unknown; children?: unknown[] };
        at += 1;
        const id = block.id ?? randomUUID();
        doc.blocks.set(id, this.blockRow(doc, id, parent, String(at), block.type, block.props, block.content, author, 1));
        if (block.children !== undefined) place(block.children, id);
      }
    };
    place(blocks, null);
    this.docs.set(doc.id, doc);
    return doc;
  }

  /** Markdown as a person types it: headings, bullets, to-dos, quotes and paragraphs. */
  private fromMarkdown(markdown: string): unknown[] {
    const text = (value: string): Inline[] => (value === '' ? [] : [{ type: 'text', text: value }]);
    return markdown
      .split('\n')
      .filter((line) => line.trim() !== '')
      .map((line) => {
        const heading = /^(#{1,6}) (.*)$/.exec(line);
        if (heading !== null) return { type: 'heading', props: { level: heading[1]?.length ?? 1 }, content: text(heading[2] ?? '') };
        const todo = /^- \[( |x)\] (.*)$/.exec(line);
        if (todo !== null) return { type: 'todo', props: { checked: todo[1] === 'x' }, content: text(todo[2] ?? '') };
        const bullet = /^[-*] (.*)$/.exec(line);
        if (bullet !== null) return { type: 'bullet', props: {}, content: text(bullet[1] ?? '') };
        const quote = /^> (.*)$/.exec(line);
        if (quote !== null) return { type: 'quote', props: {}, content: text(quote[1] ?? '') };
        return { type: 'paragraph', props: {}, content: text(line) };
      });
  }

  sync(person: Person, doc: Doc, since: number | null) {
    const changed = since === null ? [...doc.blocks.values()].filter((block) => !block.removed) : [...doc.blocks.values()].filter((block) => block.seq_updated > since);
    const discussions = since === null ? [...doc.discussions.values()] : [...doc.discussions.values()].filter((one) => one.seq_updated > since);
    return {
      document: this.full(person, doc),
      blocks: this.expandMentions(changed),
      discussions: this.expandMentions(discussions.sort((a,b) => b.seq_updated-a.seq_updated).slice(0,50).map((one) => ({ ...one, comments: one.comments.slice(-50) }))),
      seq: doc.seq,
      since,
      complete: since === null || discussions.length <= 50,
    };
  }

  // ---------------------------------------------------------------- the feed

  /** Permission writes share the same native metadata cut as content writes. */
  permissionsChanged(id: string, before: number): void {
    const doc = this.docs.get(id);
    if (doc !== undefined) this.broadcast(doc, [], [], before);
  }

  private send(watcher: Watcher, frame: Record<string, unknown>): void {
    watcher.subscriber.send({ ...frame, id: watcher.subId, feed: `doc:${watcher.doc}` });
  }

  private presence(docId: string): void {
    const here = this.watchers.filter((watcher) => watcher.doc === docId);
    const seen = new Set<string>();
    const users = here.flatMap((watcher) => {
      if (seen.has(watcher.person.me.username)) return [];
      seen.add(watcher.person.me.username);
      return [{ user: userRef(watcher.person.me), since: MADE_AT }];
    });
    for (const watcher of here) this.send(watcher, { op: 'presence', users });
  }

  /** Tells everybody who has the document open what a change touched. */
  private broadcast(doc: Doc, blocks: DocumentBlock[], discussions: DocumentDiscussion[], before: number): void {
    const lost = this.watchers.filter((watcher) => watcher.doc === doc.id && !this.holdings(watcher.person, doc).permissions.includes('doc.view'));
    if (lost.length > 0) {
      const removed = new Set(lost);
      this.watchers = this.watchers.filter((watcher) => !removed.has(watcher));
      for (const watcher of lost) this.send(watcher, { op: 'closed', reason: 'forbidden' });
      this.presence(doc.id);
    }
    if (this.lose) {
      this.lose = false;
      for (const watcher of this.watchers) if (watcher.doc === doc.id) watcher.lastSent = doc.seq;
      return;
    }
    for (const watcher of this.watchers) {
      if (watcher.doc !== doc.id) continue;
      this.send(watcher, {
        op: 'sync',
        prev: watcher.lastSent === before ? before : watcher.lastSent,
        seq: doc.seq,
        data: { document: this.ref(watcher.person, doc), blocks: this.expandMentions(blocks), discussions: this.expandMentions(discussions.slice(0,50).map((one) => ({ ...one, comments: one.comments.slice(-50) }))) },
      });
      watcher.lastSent = doc.seq;
    }
  }

  /** Handles a frame of a live connection that is about a document: subscribing, the signs of writing, and leaving. Answers whether it was. */
  frame(subscriber: Subscriber, person: Person | undefined, frame: Record<string, unknown>): boolean {
    const feed = typeof frame.feed === 'string' ? frame.feed : '';
    if (person === undefined) return false;
    if (frame.op === 'sub' && feed.startsWith('doc:')) {
      const subId = Number(frame.id);
      const doc = this.docs.get(feed.slice(4));
      if (doc === undefined || doc.deleted) {
        subscriber.send({ op: 'error', id: subId, code: 'not_found', detail: feed });
        return true;
      }
      if (!this.holdings(person, doc).permissions.includes('doc.view')) {
        subscriber.send({ op: 'error', id: subId, code: 'forbidden', detail: feed });
        return true;
      }
      const watcher: Watcher = { subscriber, subId, doc: doc.id, person, lastSent: doc.seq, signedAt: 0 };
      this.watchers.push(watcher);
      const since = typeof frame.since === 'number' ? frame.since : null;
      const reload = since !== null && since > doc.seq;
      subscriber.send({ op: 'subscribed', id: subId, feed, seq: doc.seq, mode: reload ? 'reload' : 'delta' });
      if (!reload && since !== null && since < doc.seq) {
        const rows = [...doc.blocks.values()].filter((block) => block.seq_updated > since);
        const talk = [...doc.discussions.values()].filter((one) => one.seq_updated > since);
        this.send(watcher, { op: 'sync', prev: since, seq: doc.seq, data: { document: this.ref(person, doc), blocks: clone(rows), discussions: clone(talk) } });
      }
      this.presence(doc.id);
      return true;
    }
    if (frame.op === 'unsub') {
      const watcher = this.watchers.find((one) => one.subscriber === subscriber && one.subId === Number(frame.id));
      if (watcher === undefined) return false;
      this.watchers = this.watchers.filter((one) => one !== watcher);
      subscriber.send({ op: 'closed', id: watcher.subId, feed: `doc:${watcher.doc}`, reason: 'unsubscribed' });
      this.presence(watcher.doc);
      return true;
    }
    if ((frame.op === 'typing' || frame.op === 'visible') && feed.startsWith('doc:')) {
      const watcher = this.watchers.find((one) => one.subscriber === subscriber && one.doc === feed.slice(4));
      const doc = this.docs.get(feed.slice(4));
      if (frame.op === 'typing' && watcher !== undefined && doc !== undefined && typeof frame.at === 'string') {
        const t = Date.now();
        if (this.holdings(person, doc).permissions.includes('doc.edit') && t - watcher.signedAt >= SIGN_EVERY_MS) {
          watcher.signedAt = t;
          for (const other of this.watchers) {
            if (other.doc === doc.id && other.person.me.username !== person.me.username) {
              this.send(other, { op: 'event', data: { what: 'typing', user: userRef(person.me), at: frame.at } });
            }
          }
        }
      }
      return true;
    }
    return false;
  }

  /** A live connection ended. */
  gone(subscriber: Subscriber): void {
    const gone = this.watchers.filter((watcher) => watcher.subscriber === subscriber);
    if (gone.length === 0) return;
    this.watchers = this.watchers.filter((watcher) => watcher.subscriber !== subscriber);
    for (const docId of new Set(gone.map((watcher) => watcher.doc))) this.presence(docId);
  }

  // ---------------------------------------------------------------- positions and operations

  private invalid(path: string, message: string): never {
    throw new Refusal(400, 'request.invalid', 'the request is not valid', { errors: [{ path, message }] });
  }

  /** Applies a batch whole or not at all; answers what it touched. */
  applyOps(doc: Doc, person: Person | null, author: Author, clientId: string | null, ops: DocumentOp[]) {
    const key = `${person?.me.username ?? 'agent'}:${clientId ?? ''}`;
    if (clientId !== null && doc.done.has(key)) return doc.done.get(key) as { document: DocumentRef; blocks: DocumentBlock[]; seq: number };
    if (ops.length > OPS_MAX) throw new Refusal(413, 'docs.too_large', 'too many operations', { limit: 'operations' });

    // Conflicts are looked for first, against what is committed: every block that changed or went is named.
    const current = new Map<string, DocumentBlock>();
    const checks = (id: string, base?: number) => {
      const held = doc.blocks.get(id);
      const gone: DocumentBlock = { id, document_id: doc.id, parent_id: null, position: '0', type: 'paragraph', props: {}, content: [], removed: true, seq_created: 0, seq_updated: doc.seq, updated_by: null, updated_at: now() };
      if (held === undefined || held.removed || (base !== undefined && held.seq_updated !== base)) current.set(id, held ?? gone);
    };
    for (const op of ops) {
      if (op.op === 'update') checks(op.id, op.base_seq);
      else if (op.op === 'move') {
        checks(op.id);
        if (op.parent_id !== null) checks(op.parent_id);
      } else if (op.op === 'insert' && op.parent_id !== null) {
        // A parent made earlier in the same batch is no conflict.
        const madeBefore = ops.some((other) => other.op === 'insert' && other.block.id === op.parent_id);
        if (!madeBefore) checks(op.parent_id);
      }
    }
    if (current.size > 0) throw new Refusal(409, 'conflict.stale_revision', 'a block changed meanwhile', { current: clone([...current.values()]) });

    const before = doc.seq;
    const seq = before + 1;
    const work = new Map([...doc.blocks].map(([id, block]) => [id, clone(block)]));
    const touched = new Set<string>();
    const tally = { inserted: 0, updated: 0, moved: 0, removed: 0 };
    const positionIn = (parent: string | null, afterId: string | null): string => {
      const siblings = [...work.values()].filter((block) => !block.removed && block.parent_id === parent).sort((one, other) => Number(one.position) - Number(other.position));
      if (afterId === null) return siblings.length === 0 ? '1' : String(Number(siblings[0]?.position) - 1);
      const at = siblings.findIndex((block) => block.id === afterId);
      if (at < 0) return siblings.length === 0 ? '1' : String(Number(siblings[siblings.length - 1]?.position) + 1);
      const next = siblings[at + 1];
      const here = Number(siblings[at]?.position);
      return next === undefined ? String(here + 1) : String((here + Number(next.position)) / 2);
    };
    const markRemoved = (id: string) => {
      const block = work.get(id);
      if (block === undefined || block.removed) return;
      block.removed = true;
      block.seq_updated = seq;
      block.updated_by = author;
      touched.add(id);
      tally.removed += 1;
      for (const child of [...work.values()]) if (child.parent_id === id) markRemoved(child.id);
    };
    for (const [index, op] of ops.entries()) {
      switch (op.op) {
        case 'insert': {
          const spec = op.block;
          if (!BLOCK_TYPES.has(spec.type)) this.invalid(`ops[${index}].block.type`, 'is not a block type');
          if (work.has(spec.id)) throw new Refusal(409, 'conflict.exists', 'a block with that identifier exists');
          const parent = op.parent_id === null ? undefined : work.get(op.parent_id);
          if (parent !== undefined && LEAF_TYPES.has(parent.type)) this.invalid(`ops[${index}].parent_id`, 'is a block that may not have children');
          const place = (item: typeof spec, at: string | null, afterId: string | null) => {
            const position = positionIn(at, afterId);
            work.set(item.id, this.blockRow(doc, item.id, at, position, item.type, item.props, item.content, author, seq));
            touched.add(item.id);
            tally.inserted += 1;
            let previous: string | null = null;
            for (const child of item.children ?? []) {
              place(child, item.id, previous);
              previous = child.id;
            }
          };
          place(spec, op.parent_id, op.after_id);
          break;
        }
        case 'update': {
          const block = work.get(op.id);
          if (block === undefined) break;
          if (op.type !== undefined && op.type !== null) {
            if (!BLOCK_TYPES.has(op.type)) this.invalid(`ops[${index}].type`, 'is not a block type');
            block.type = op.type;
          }
          if (op.props !== undefined && op.props !== null) block.props = op.props;
          if (op.content !== undefined && op.content !== null) block.content = op.content;
          block.seq_updated = seq;
          block.updated_by = author;
          block.updated_at = now();
          touched.add(op.id);
          tally.updated += 1;
          break;
        }
        case 'move': {
          const block = work.get(op.id);
          if (block === undefined) break;
          for (let up = op.parent_id === null ? undefined : work.get(op.parent_id); up !== undefined; up = up.parent_id === null ? undefined : work.get(up.parent_id)) {
            if (up.id === op.id) this.invalid(`ops[${index}].parent_id`, 'is under the block itself');
          }
          block.parent_id = op.parent_id;
          block.position = positionIn(op.parent_id, op.after_id);
          block.seq_updated = seq;
          block.updated_by = author;
          touched.add(op.id);
          tally.moved += 1;
          break;
        }
        case 'remove':
          markRemoved(op.id);
          break;
        default:
      }
    }
    const liveCount = [...work.values()].filter((block) => !block.removed).length;
    if (liveCount > doc.limits.blocks) throw new Refusal(413, 'docs.too_large', 'the document is as large as one may be', { limit: 'blocks' });
    for (const block of work.values()) {
      if (block.removed || !touched.has(block.id)) continue;
      if (JSON.stringify(block.content).length + JSON.stringify(block.props).length > 64 * 1024) throw new Refusal(413, 'docs.too_large', 'this block is too long', { limit: 'block_bytes' });
    }

    this.cutIfDue(doc);
    doc.blocks = work;
    doc.seq = seq;
    doc.updatedAt = now();
    doc.updatedBy = author;
    this.track(doc, author, tally);
    const rows = [...touched].map((id) => doc.blocks.get(id)).filter((block): block is DocumentBlock => block !== undefined);
    const reply = { document: this.ref(person ?? (this.world.people.values().next().value!), doc), blocks: this.expandMentions(rows), seq };
    if (clientId !== null) doc.done.set(key, reply);
    this.broadcast(doc, rows, [], before);
    return reply;
  }

  // ---------------------------------------------------------------- versions

  private track(doc: Doc, author: Author, tally: Omit<Span, 'authors' | 'from' | 'to'>): void {
    const at = now();
    doc.span ??= { authors: new Map(), inserted: 0, updated: 0, moved: 0, removed: 0, from: at, to: at };
    doc.span.authors.set('id' in author ? author.id : `agent:${author.agent}`, author);
    doc.span.inserted += tally.inserted;
    doc.span.updated += tally.updated;
    doc.span.moved += tally.moved;
    doc.span.removed += tally.removed;
    doc.span.to = at;
  }

  /** Cuts a version of the document as it stands: what was changed since the last cut becomes one entry. */
  cut(doc: Doc, cause: NonNullable<DocumentVersion['cause']>): void {
    if (doc.span === null) return;
    doc.versions.push({
      version: doc.versions.length + 1,
      title: doc.title,
      blocks: clone(this.live(doc)),
      cause,
      authors: [...doc.span.authors.values()],
      changed: { inserted: doc.span.inserted, updated: doc.span.updated, moved: doc.span.moved, removed: doc.span.removed },
      from: doc.span.from,
      to: doc.span.to,
    });
    doc.span = null;
  }

  /** A version is cut while a document is being written once the last is older than ten minutes. */
  private cutIfDue(doc: Doc): void {
    const last = doc.versions[doc.versions.length - 1];
    const since = last === undefined ? Date.parse(doc.createdAt) : Date.parse(last.to);
    if (doc.span !== null && Date.now() - since > 10 * 60_000 && Date.now() - Date.parse(doc.span.from) > 10 * 60_000) this.cut(doc, 'window');
  }

  private versionRef(version: Version): DocumentVersion {
    return {
      version: version.version,
      title: version.title,
      bytes: JSON.stringify(version.blocks).length,
      block_count: version.blocks.length,
      cause: version.cause,
      authors: version.authors,
      changed: version.changed,
      from: version.from,
      to: version.to,
    };
  }

  private versions(doc: Doc, query: URLSearchParams) {
    const author = query.get('author');
    const authored = (authors: Author[]): boolean =>
      author === null || authors.some((one) => ('id' in one ? one.id === author : `thread:${one.thread.id}` === author));
    const items: DocumentVersion[] = [];
    if (doc.span !== null && authored([...doc.span.authors.values()])) {
      items.push({
        version: null,
        title: doc.title,
        bytes: this.bytesOf(doc),
        block_count: this.live(doc).length,
        cause: null,
        authors: [...doc.span.authors.values()],
        changed: { inserted: doc.span.inserted, updated: doc.span.updated, moved: doc.span.moved, removed: doc.span.removed },
        from: doc.span.from,
        to: doc.span.to,
      });
    }
    for (const version of [...doc.versions].reverse()) if (authored(version.authors)) items.push(this.versionRef(version));
    const limit = Number(query.get('limit') ?? 20);
    const start = Number(query.get('cursor') ?? 0);
    return { items: items.slice(start, start + limit), next_cursor: start + limit < items.length ? String(start + limit) : null };
  }

  private snapshotOf(doc: Doc, from: number): DocumentBlock[] {
    if (from <= 0) return [];
    const held = doc.versions.find((version) => version.version === from);
    if (held === undefined) throw new Refusal(404, 'not_found', 'no such version');
    return held.blocks;
  }

  private changes(person: Person, doc: Doc, query: URLSearchParams): DocumentChanges {
    const fromText = query.get('from') ?? '0';
    const from = fromText === 'seen' ? (doc.seen.get(person.me.username)?.version ?? 0) : Number(fromText);
    const toText = query.get('to');
    const to = toText === null ? null : Number(toText);
    const before = new Map(this.snapshotOf(doc, from).map((block) => [block.id, block]));
    const after = new Map((to === null ? this.live(doc) : this.snapshotOf(doc, to)).map((block) => [block.id, block]));
    const blocks: DocumentChanges['blocks'] = [];
    const authors = new Map<string, Author>();
    for (const id of new Set([...before.keys(), ...after.keys()])) {
      const one = before.get(id) ?? null;
      const other = after.get(id) ?? null;
      const same =
        one !== null &&
        other !== null &&
        one.type === other.type &&
        one.parent_id === other.parent_id &&
        one.position === other.position &&
        JSON.stringify(one.props) === JSON.stringify(other.props) &&
        JSON.stringify(one.content) === JSON.stringify(other.content);
      if (same) continue;
      const by = other?.updated_by ?? one?.updated_by ?? null;
      if (by !== null) authors.set('id' in by ? by.id : `agent:${by.agent}`, by);
      blocks.push({ id, before: one, after: other, by });
    }
    const titleBefore = from <= 0 ? null : (doc.versions.find((version) => version.version === from)?.title ?? null);
    const titleAfter = to === null ? doc.title : (doc.versions.find((version) => version.version === to)?.title ?? doc.title);
    return {
      from,
      to,
      title: titleBefore !== null && titleBefore !== titleAfter ? { before: titleBefore, after: titleAfter } : null,
      blocks,
      authors: [...authors.values()],
    };
  }

  // ---------------------------------------------------------------- discussions

  private stamp(doc: Doc, person: Person): number {
    doc.seq += 1;
    doc.updatedBy = doc.updatedBy ?? this.authorOf(person);
    return doc.seq;
  }

  private anchored(doc: Doc, discussion: DocumentDiscussion): DocumentDiscussion {
    const block = discussion.block_id === null ? undefined : doc.blocks.get(discussion.block_id);
    if (block === undefined || block.removed) return { ...discussion, block_id: null, anchored: false };
    const text = JSON.stringify(block.content);
    return { ...discussion, anchored: discussion.quote === '' || text.includes(JSON.stringify(discussion.quote).slice(1, -1)) };
  }

  private discussionOf(doc: Doc, id: string): DocumentDiscussion {
    const found = doc.discussions.get(id);
    if (found === undefined) throw new Refusal(404, 'not_found', 'no such discussion');
    return found;
  }

  private publish(doc: Doc, discussion: DocumentDiscussion, before: number): void {
    doc.discussions.set(discussion.id, discussion);
    this.broadcast(doc, [], [discussion], before);
  }

  private startDiscussion(person: Person, doc: Doc, body: Record<string, unknown>): DocumentDiscussion {
    this.need(person, doc, 'doc.comment', true);
    const block = doc.blocks.get(asText(body.block_id));
    if (block === undefined || block.removed) throw new Refusal(404, 'not_found', 'no such block');
    const before = doc.seq;
    const seq = this.stamp(doc, person);
    const content = (Array.isArray(body.content) ? body.content : []) as Inline[];
    const id = randomUUID();
    const discussion: DocumentDiscussion = {
      id,
      document_id: doc.id,
      block_id: block.id,
      quote: asText(body.quote).slice(0, 400),
      anchored: true,
      resolved: null,
      created_by: userRef(person.me),
      created_at: now(),
      comments: [{ id: randomUUID(), discussion_id: id, author: userRef(person.me), content, reactions: [], edited_at: null, deleted: false, created_at: now(), seq_updated: seq }],
      seq_updated: seq,
    };
    this.publish(doc, this.anchored(doc, discussion), before);
    return clone(doc.discussions.get(id) ?? discussion);
  }

  private answer(person: Person, doc: Doc, discussionId: string, body: Record<string, unknown>): DocumentComment {
    this.need(person, doc, 'doc.comment', true);
    const held = this.discussionOf(doc, discussionId);
    const before = doc.seq;
    const seq = this.stamp(doc, person);
    const comment: DocumentComment = {
      id: randomUUID(),
      discussion_id: held.id,
      author: userRef(person.me),
      content: (Array.isArray(body.content) ? body.content : []) as Inline[],
      reactions: [],
      edited_at: null,
      deleted: false,
      created_at: now(),
      seq_updated: seq,
    };
    this.publish(doc, { ...held, comments: [...held.comments, comment], seq_updated: seq }, before);
    return clone(comment);
  }

  private resolve(person: Person, doc: Doc, discussionId: string, resolved: boolean): DocumentDiscussion {
    const held = this.discussionOf(doc, discussionId);
    const wrote = held.comments.some((comment) => comment.author.id === person.me.id);
    if (!wrote) this.need(person, doc, 'doc.edit', true);
    else this.need(person, doc, 'doc.view', true);
    const before = doc.seq;
    const seq = this.stamp(doc, person);
    this.publish(doc, { ...held, resolved: resolved ? { at: now(), by: userRef(person.me) } : null, seq_updated: seq }, before);
    return clone(doc.discussions.get(held.id) ?? held);
  }

  private commentOf(doc: Doc, commentId: string): { discussion: DocumentDiscussion; comment: DocumentComment } {
    for (const discussion of doc.discussions.values()) {
      const comment = discussion.comments.find((one) => one.id === commentId);
      if (comment !== undefined) return { discussion, comment };
    }
    throw new Refusal(404, 'not_found', 'no such comment');
  }

  private changeComment(person: Person, doc: Doc, commentId: string, change: (comment: DocumentComment) => DocumentComment | null, mayDelete = false): DocumentComment {
    const { discussion, comment } = this.commentOf(doc, commentId);
    const own = comment.author.id === person.me.id;
    if (!own && !(mayDelete && this.holdings(person, doc).permissions.includes('doc.manage'))) this.deny(person, doc, mayDelete ? 'doc.manage' : 'doc.comment');
    if (this.isArchived(doc)) throw new Refusal(409, 'node.archived', 'the place is archived');
    const before = doc.seq;
    const seq = this.stamp(doc, person);
    const next = change({ ...comment, seq_updated: seq }) ?? { ...comment, seq_updated: seq };
    this.publish(doc, { ...discussion, comments: discussion.comments.map((one) => (one.id === commentId ? next : one)), seq_updated: seq }, before);
    return clone(next);
  }

  // ---------------------------------------------------------------- export

  /** What a document says in a format, for the file the export part writes: its words as that format has them, and what the format cannot keep of its blocks. */
  rendered(person: Person, doc: Doc, format: string, withComments: boolean): { text: string; type: string; extension: string; losses: string[] } {
    const rows = this.live(doc).sort((one, other) => Number(one.position) - Number(other.position));
    const losses: string[] = [];
    const callouts = rows.filter((block) => block.type === 'callout').length;
    if (callouts > 0) losses.push(`${callouts} callout${callouts === 1 ? '' : 's'} become quotes`);
    if (rows.some((block) => block.type === 'columns')) losses.push('columns become one column');
    let text: string;
    let type: string;
    let extension: string;
    if (format === 'markdown') {
      text = [`# ${doc.title}`, ...rows.map((block) => (block.type === 'heading' ? `${'#'.repeat((block.props as { level?: number }).level ?? 1)} ${wordsOf(block)}` : block.type === 'bullet' ? `- ${wordsOf(block)}` : block.type === 'todo' ? `- [${(block.props as { checked?: boolean }).checked === true ? 'x' : ' '}] ${wordsOf(block)}` : wordsOf(block)))].join('\n\n');
      type = 'text/markdown';
      extension = 'md';
    } else if (format === 'html') {
      text = `<!doctype html><h1>${doc.title}</h1>${rows.map((block) => `<p>${wordsOf(block)}</p>`).join('')}`;
      type = 'text/html';
      extension = 'html';
    } else if (format === 'typst') {
      text = `= ${doc.title}\n\n${rows.map(wordsOf).join('\n\n')}`;
      type = 'text/plain';
      extension = 'typ';
    } else {
      text = JSON.stringify({ document: this.ref(person, doc), blocks: rows });
      type = 'application/json';
      extension = 'json';
    }
    if (withComments) text += `\n\n[comments: ${doc.discussions.size}]`;
    return { text, type, extension, losses: format === 'markdown' ? losses : [] };
  }

  /** The documents a person may read, for the parts that make something of them (the export of a place). */
  readable(person: Person): Doc[] {
    return [...this.docs.values()].filter((doc) => !doc.deleted && this.holdings(person, doc).permissions.includes('doc.view'));
  }

  // ---------------------------------------------------------------- the requests

  /** Answers a request about documents, or `null` when it is not one. */
  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    const tree = this.treeHandle(person, method, path, query, body);
    if (tree !== null) return tree;
    const route = `${method} ${path}`;
    let found: RegExpExecArray | null;

    if ((found = /^GET \/nodes\/([^/]+)\/documents$/.exec(route))) return { status: 200, body: this.list(person, found[1] ?? '', query) };
    if ((found = /^POST \/nodes\/([^/]+)\/documents$/.exec(route))) {
      const nodeId = found[1] ?? '';
      this.world.places.need(person, nodeId, `${this.node(nodeId).kind}.view`);
      const title = asText(body.title).trim();
      if (title === '' || title.length > 200) this.invalid('title', 'must be 1 to 200 characters');
      if (body.blocks !== undefined && body.markdown !== undefined) this.invalid('markdown', 'is given beside blocks: name one of them');
      const blocks = Array.isArray(body.blocks) ? body.blocks : typeof body.markdown === 'string' ? this.fromMarkdown(body.markdown) : [];
      const doc = this.make(person, nodeId, title, blocks, body.access === 'private' ? 'private' : 'follows');
      return { status: 201, body: this.full(person, doc) };
    }

    if (route === 'POST /documents/convert') {
      // The server's one reader, in the small: the Markdown reader of this double, and the shared HTML converter itself.
      const text = asText(body.text);
      const read = body.format === 'html' ? readHtml(text) : { pieces: body.format === 'text' ? [{ type: 'paragraph', props: {}, content: [{ type: 'text', text }], children: [] } satisfies Piece] : readMarkdown(text), warnings: [] as string[] };
      const named = (pieces: readonly Piece[]): unknown[] => pieces.map((one) => ({ id: randomUUID(), type: one.type, props: one.props, content: one.content, children: named(one.children) }));
      return { status: 200, body: { blocks: named(read.pieces), warnings: read.warnings.map((message) => ({ code: 'reader.other', params: {}, message })), files: [] } };
    }

    if (!path.startsWith('/documents/')) return null;
    const rest = /^\/documents\/([^/]+)(\/.*)?$/.exec(path);
    const id = rest?.[1] ?? '';
    const tail = rest?.[2] ?? '';
    const restoring = route === `POST /documents/${id}/restore`;
    const doc = this.find(person, id, restoring);

    switch (`${method} ${tail}`) {
      case 'GET /links':
        return { status: 200, body: this.linksOf(person, doc) };
      case 'PUT /links': {
        this.need(person, doc, 'doc.manage');
        if (this.linkLocks.has(doc.id)) return { status: 409, body: { code: 'access.locked', detail: 'a lock above decides which tabs documents here show' } };
        const shown = body.shown;
        if (shown === null) this.linkTabs.delete(doc.id);
        else if (Array.isArray(shown)) this.linkTabs.set(doc.id, LINK_TABS.filter((tab) => shown.includes(tab)));
        else this.invalid('shown', 'is a list of tabs or null');
        return { status: 200, body: this.linksOf(person, doc) };
      }
      case 'GET /references':
        return { status: 200, body: this.referencesOf(person, doc, query) };
      case 'GET /graph':
        return { status: 200, body: this.graphOf(person, doc, query) };
      case 'GET /related': {
        const configured = this.relatedAnswers.get(doc.id);
        if (configured !== undefined) return clone(configured);
        const answer: RelatedItems = {
          next_cursor: null, seq: doc.seq, meaning: 'off', meaning_reason: null,
          items: [...this.docs.values()]
            .filter((candidate) => candidate.id !== doc.id && !candidate.deleted && candidate.nodeId === doc.nodeId && this.holdings(person, candidate).permissions.includes('doc.view'))
            .sort((left, right) => right.updatedAt.localeCompare(left.updatedAt))
            .slice(0, RELATED_ITEMS_DEFAULT)
            .map((candidate) => ({ kind: 'document', id: candidate.id, name: candidate.title, context: this.path(candidate.nodeId).map((node) => ({ id: node.id, name: node.name })), related_by: ['place'], place: 'topic', references: null })),
        };
        return { status: 200, body: answer };
      }
      case 'GET ':
        return { status: 200, body: this.full(person, doc) };
      case 'GET /sync': {
        const since = query.get('since');
        return { status: 200, body: this.sync(person, doc, since === null ? null : Number(since)) };
      }
      case 'PATCH ': {
        this.need(person, doc, 'doc.edit', true);
        if (body.title === undefined && body.description === undefined) this.invalid('title', 'give a title, a description, or both');
        const title = body.title === undefined ? doc.title : asText(body.title).trim();
        if (title === '' || title.length > 200) this.invalid('title', 'must be 1 to 200 characters');
        if (body.description !== undefined && asText(body.description).length > 4000) this.invalid('description', 'must be at most 4000 characters');
        const before = doc.seq;
        doc.title = title;
        if (body.description !== undefined) {
          doc.description = asText(body.description);
          doc.descriptionSource = 'user';
        }
        doc.seq += 1;
        doc.updatedAt = now();
        doc.updatedBy = this.authorOf(person);
        this.broadcast(doc, [], [], before);
        return { status: 200, body: this.full(person, doc) };
      }
      case 'DELETE ':
        this.need(person, doc, 'doc.manage', true);
        doc.deleted = true;
        return { status: 204 };
      case 'POST /restore': {
        this.need(person, doc, 'doc.manage');
        if (!doc.deleted) throw new Refusal(404, 'not_found', 'the document is not deleted');
        doc.deleted = false;
        return { status: 200, body: this.full(person, doc) };
      }
      case 'POST /ops': {
        this.need(person, doc, 'doc.edit', true);
        if (typeof body.client_id !== 'string') this.invalid('client_id', 'is required');
        return { status: 200, body: this.applyOps(doc, person, this.authorOf(person), asText(body.client_id), (Array.isArray(body.ops) ? body.ops : []) as DocumentOp[]) };
      }
      case 'POST /seen': {
        doc.seen.set(person.me.username, { seq: Number(body.seq), version: doc.versions.length });
        return { status: 204 };
      }
      case 'GET /versions':
        return { status: 200, body: this.versions(doc, query) };
      case 'GET /changes':
        return { status: 200, body: this.changes(person, doc, query) };
      case 'GET /discussions': {
        const mode = query.get('resolved') ?? 'exclude';
        const items = [...doc.discussions.values()]
          .filter((one) => (mode === 'include' ? true : mode === 'only' ? one.resolved !== null : one.resolved === null))
          .sort((one, other) => other.seq_updated - one.seq_updated);
        const after = query.get('cursor');
        const start = after === null ? 0 : items.findIndex((one) => one.id === after) + 1;
        const limit = Math.min(200, Math.max(1, Number(query.get('limit') ?? 50)));
        const shown = items.slice(start,start+limit);
        return { status: 200, body: { items: this.expandMentions(shown.map((one) => ({ ...one, comments: one.comments.slice(-50) }))), next_cursor: start+limit<items.length ? shown.at(-1)?.id ?? null : null } };
      }
      case 'POST /discussions':
        return { status: 201, body: this.expandMentions(this.startDiscussion(person, doc, body)) };
      case 'POST /move/preview':
        return { status: 200, body: { gaining: [], losing: [], more: 0, pages: 1 } };
      case 'POST /move': {
        this.need(person, doc, 'doc.manage', true);
        const target = asText(body.node_id);
        this.world.places.need(person, target, `${this.node(target).kind}.view`);
        doc.nodeId = target;
        doc.seq += 1;
        return { status: 200, body: this.full(person, doc) };
      }
      case 'PATCH /permissions': {
        this.need(person, doc, 'access.manage');
        doc.page = body.whole === 'private' ? 'restricted' : 'follows';
        doc.seq += 1;
        return { status: 200, body: { revision: doc.seq } };
      }
      case 'POST /permissions/preview':
        return { status: 200, body: { gaining: [], losing: [], more: 0, pages: 1 } };
      case 'POST /files':
        // Files wait for the blob store of the real system.
        throw new Refusal(404, 'not_found', 'files of a document are not served yet');
      default:
    }

    if ((found = /^GET \/discussions\/([^/]+)\/comments$/.exec(`${method} ${tail}`))) {
      const discussion = this.discussionOf(doc, found[1] ?? '');
      const all = [...discussion.comments].sort((a,b) => b.created_at.localeCompare(a.created_at) || b.id.localeCompare(a.id));
      const after = query.get('cursor');
      const start = after === null ? 0 : all.findIndex((one) => one.id === after)+1;
      const limit = Math.min(200, Math.max(1, Number(query.get('limit') ?? 50)));
      const shown = all.slice(start,start+limit);
      return { status: 200, body: { items: this.expandMentions([...shown].reverse()), next_cursor: start+limit<all.length ? shown.at(-1)?.id ?? null : null, wrote: all.some((one) => one.author.id === person.me.id) } };
    }
    if ((found = /^POST \/discussions\/([^/]+)\/comments$/.exec(`${method} ${tail}`))) return { status: 201, body: this.expandMentions(this.answer(person, doc, found[1] ?? '', body)) };
    if ((found = /^POST \/discussions\/([^/]+)\/resolve$/.exec(`${method} ${tail}`))) return { status: 200, body: this.resolve(person, doc, found[1] ?? '', body.resolved === true) };
    if ((found = /^PATCH \/comments\/([^/]+)$/.exec(`${method} ${tail}`))) {
      return { status: 200, body: this.expandMentions(this.changeComment(person, doc, found[1] ?? '', (comment) => ({ ...comment, content: (Array.isArray(body.content) ? body.content : []) as Inline[], edited_at: now() }))) };
    }
    if ((found = /^DELETE \/comments\/([^/]+)$/.exec(`${method} ${tail}`))) {
      this.changeComment(person, doc, found[1] ?? '', (comment) => ({ ...comment, content: [], deleted: true }), true);
      return { status: 204 };
    }
    if ((found = /^PUT \/comments\/([^/]+)\/reaction$/.exec(`${method} ${tail}`))) {
      this.need(person, doc, 'doc.comment', true);
      const emoji = asText(body.emoji);
      return {
        status: 200,
        body: this.changeComment(person, doc, found[1] ?? '', (comment) => {
          const others = comment.reactions.filter((reaction) => reaction.emoji !== emoji);
          const mine = comment.reactions.find((reaction) => reaction.emoji === emoji)?.users ?? [];
          const users = body.on === true ? [...new Set([...mine, person.me.id])] : mine.filter((user) => user !== person.me.id);
          return { ...comment, reactions: users.length === 0 ? others : [...others, { emoji, users }] };
        }, true),
      };
    }
    if ((found = /^GET \/versions\/([^/]+)$/.exec(`${method} ${tail}`))) {
      const version = doc.versions.find((one) => String(one.version) === found?.[1]);
      if (version === undefined) throw new Refusal(404, 'not_found', 'no such version');
      return { status: 200, body: { version: this.versionRef(version), blocks: this.expandMentions(version.blocks) } };
    }
    if ((found = /^POST \/versions\/([^/]+)\/restore$/.exec(`${method} ${tail}`))) {
      this.need(person, doc, 'doc.manage', true);
      const version = doc.versions.find((one) => String(one.version) === found?.[1]);
      if (version === undefined) throw new Refusal(404, 'not_found', 'no such version');
      // The present state is kept as a version first; then the blocks of that version stand again.
      const author = this.authorOf(person);
      this.track(doc, author, { inserted: 0, updated: 0, moved: 0, removed: 0 });
      this.cut(doc, 'restore');
      const before = doc.seq;
      const seq = before + 1;
      const rows: DocumentBlock[] = [];
      for (const block of doc.blocks.values()) {
        if (!block.removed && !version.blocks.some((one) => one.id === block.id)) {
          block.removed = true;
          block.seq_updated = seq;
          rows.push(block);
        }
      }
      for (const old of version.blocks) {
        const row = { ...clone(old), removed: false, seq_updated: seq, updated_by: author };
        doc.blocks.set(row.id, row);
        rows.push(row);
      }
      doc.seq = seq;
      doc.title = version.title;
      doc.updatedAt = now();
      doc.updatedBy = author;
      this.broadcast(doc, rows, [], before);
      return { status: 200, body: this.sync(person, doc, null) };
    }
    return null;
  }

  // ---------------------------------------------------------------- what only a test can do

  /** The requests only the double has: play other people, cut a version, lose a frame. */
  control(path: string, body: Record<string, unknown>): Answer | null {
    const doc = this.docs.get(asText(body.doc));
    switch (path) {
      case '/related':
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        this.relatedAnswers.set(doc.id, { status: typeof body.status === 'number' ? body.status : 200, body: body.answer });
        return { status: 204 };
      case '/links-lock':
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        if (body.locked === false) this.linkLocks.delete(doc.id);
        else this.linkLocks.add(doc.id);
        return { status: 204 };
      case '/lose':
        this.lose = true;
        return { status: 204 };
      case '/as': {
        // Another person's batch, sent as they would send it.
        const person = this.person(asText(body.user));
        if (doc === undefined || person === undefined) return { status: 404, body: { detail: 'no such document or person' } };
        const reply = this.applyOps(doc, person, this.authorOf(person), asText(body.client_id) === '' ? randomUUID() : asText(body.client_id), body.ops as DocumentOp[]);
        return { status: 200, body: reply };
      }
      case '/agent': {
        // An agent that was let write: its blocks say which agent changed them.
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        const thread = this.nodeRef(asText(body.thread));
        const reply = this.applyOps(doc, null, { agent: asText(body.agent), thread }, null, body.ops as DocumentOp[]);
        return { status: 200, body: reply };
      }
      case '/cut':
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        this.cut(doc, 'idle');
        return { status: 204 };
      case '/limits':
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        doc.limits = { ...doc.limits, ...(body.limits as Partial<Document['limits']>) };
        return { status: 204 };
      case '/archived':
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        doc.archivedOverride = body.archived === true ? true : body.archived === false ? false : null;
        return { status: 204 };
      case '/blocks':
        // What the server holds, for a test to read: the live blocks in the order of the page.
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        return { status: 200, body: { blocks: clone(this.live(doc)), seq: doc.seq, title: doc.title, versions: doc.versions.length } };
      case '/files':
        if (doc === undefined) return { status: 404, body: { detail: 'no such document' } };
        doc.files = clone(body.files as DocumentFile[]);
        return { status: 204 };
      case '/make': {
        // A document of a given size, for the budgets of a long page: `count` paragraphs of a sentence each. `private` makes it one that follows nobody's page.
        const person = this.person(asText(body.user) === '' ? 'ada' : asText(body.user));
        if (person === undefined) return { status: 404, body: { detail: 'no such person' } };
        const count = Number(body.count ?? 10);
        const blocks = Array.from({ length: count }, (_, index) => ({ type: 'paragraph', props: {}, content: [{ type: 'text', text: `Paragraph ${index + 1}: the quick brown fox jumps over the lazy dog.` }] }));
        const made = this.make(person, asText(body.node) === '' ? fixtureId(3, 3) : asText(body.node), asText(body.title) === '' ? 'A long page' : asText(body.title), blocks, body.private === true ? 'private' : 'follows');
        return { status: 200, body: { id: made.id } };
      }
      case '/ids':
        return { status: 200, body: { documents: [...this.docs.values()].map((one) => ({ id: one.id, title: one.title, node: one.nodeId })) } };
      default:
        return null;
    }
  }

  // ---------------------------------------------------------------- the world as a team at work

  /** Gives the team's topic a document with a block of every kind, and a second that somebody else wrote. */
  seedTeam(): void {
    const ada = this.world.people.get('ada');
    const dora = this.world.people.get('dora');
    if (ada === undefined || dora === undefined) return;
    const text = (value: string, extra: object = {}): Inline => ({ type: 'text', text: value, ...extra });
    const rows = (...lines: string[][]): TableRow[] => lines.map((line) => ({ cells: line.map((cell) => ({ content: [text(cell)] })) }));
    const b = (n: number) => fixtureId(8, n);
    const plan = this.make(
      ada,
      fixtureId(3, 3),
      'Rollout plan',
      [
        { id: b(1), type: 'heading', props: { level: 1 }, content: [text('Rollout plan')] },
        { id: b(2), type: 'paragraph', props: {}, content: [text('Ship the redirect fix behind the flag, '), text('then', { marks: ['bold'] }), text(':')] },
        { id: b(3), type: 'todo', props: { checked: false }, content: [text('Canary on staging')] },
        { id: b(4), type: 'todo', props: { checked: true }, content: [text("Announce in the topic's chat")] },
        { id: b(5), type: 'toggle', props: {}, content: [text('Rollback steps')], children: [{ id: b(6), type: 'numbered', props: {}, content: [text('Disable the flag')] }, { id: b(7), type: 'numbered', props: {}, content: [text('Restart the workers')] }] },
        { id: b(8), type: 'callout', props: { tone: 'warning' }, content: [text('Do not enable before the migration has run.')] },
        { id: b(9), type: 'quote', props: {}, content: [text('Sessions end too early.')] },
        { id: b(10), type: 'bullet', props: {}, content: [text('A bulleted line')] },
        { id: b(11), type: 'code', props: { language: 'ts' }, content: [text('const flag = true;')] },
        { id: b(12), type: 'math', props: { source: 'E = mc^2' } },
        { id: b(13), type: 'diagram', props: { source: 'graph TD; A-->B' } },
        { id: b(14), type: 'divider', props: {} },
        { id: b(15), type: 'table', props: { header_row: true }, content: rows(['Step', 'Owner'], ['Canary', 'Dora']) },
        { id: b(16), type: 'columns', props: {}, children: [{ id: b(17), type: 'column', props: {}, children: [{ id: b(18), type: 'paragraph', props: {}, content: [text('Left column')] }] }, { id: b(19), type: 'column', props: {}, children: [{ id: b(20), type: 'paragraph', props: {}, content: [text('Right column')] }] }] },
        { id: b(21), type: 'video', props: { url: 'https://media.example.org/demo.mp4' }, content: [text('Demo')] },
        { id: b(22), type: 'audio', props: { url: 'https://media.example.org/talk.mp3' }, content: [] },
        { id: b(24), type: 'image', props: { file_id: fixtureId(10, 1), align: 'center' }, content: [text('A caption')] },
        { id: b(25), type: 'file', props: { file_id: fixtureId(10, 2) }, content: [text('data set')] },
        { id: b(23), type: 'page_break', props: {} },
      ],
      'follows',
    );
    plan.updatedAt = '2026-10-01T09:20:00.000Z';
    plan.seq = 1;
    const notes = this.make(dora, fixtureId(3, 3), 'Notes from the canary', [{ id: b(30), type: 'paragraph', props: {}, content: [text('Nothing odd so far.')] }], 'follows');
    notes.updatedAt = '2026-10-01T09:10:00.000Z';
    // The private page is the third, as the documents' own tests name it.
    const mine = this.make(ada, fixtureId(3, 3), 'Only for me', [{ id: b(40), type: 'paragraph', props: {}, content: [text('Private thoughts.')] }], 'private');
    mine.updatedAt = '2026-10-01T09:05:00.000Z';
    // Beside other places, so that the topics the documents' own tests count on (the first with its pages, a second with none) keep what they have: what the finder lists, the search finds and the export of a place takes.
    const grace = this.world.people.get('grace');
    if (grace !== undefined) {
      const checklist = this.make(
        ada,
        ACME.web,
        'Release checklist',
        [
          { id: b(50), type: 'paragraph', props: {}, content: [text('Check that the redirect after login still sends people to the page they came from.')] },
          { id: b(51), type: 'paragraph', props: {}, content: [text('Tag the release and tell the team in the chat.')] },
        ],
        'follows',
        { id: fixtureId(0x30, 1) },
      );
      checklist.updatedAt = '2026-10-01T09:10:00.000Z';
      const idle = this.make(
        grace,
        ACME.infra,
        'Idle-stop notes',
        [{ id: b(52), type: 'paragraph', props: {}, content: [text('An environment that nobody has used for a day is stopped by the idle rule.')] }],
        'follows',
        { id: fixtureId(0x30, 2) },
      );
      idle.updatedAt = '2026-10-01T09:10:00.000Z';
      // Documents of a size, with files and a past, where the archive of a place goes through them: what each is said to weigh is what a plan adds up.
      const KIB = 1024;
      const kept = (id: number, node: string, title: string, owner: Person, said: Doc['said'], files: { name: string; size: number; media_type: string }[] = []) =>
        this.make(owner, node, title, [{ id: b(60 + id), type: 'paragraph', props: {}, content: [text(`${title}.`)] }], 'follows', { id: fixtureId(0x30, id), said, files });
      kept(3, ACME.web, 'Web handbook', ada, { bytes: 6 * KIB, version: 4, open: 2 }, [
        { name: 'plan.csv', size: 40 * KIB, media_type: 'text/csv' },
        { name: 'diagram.png', size: 90 * KIB, media_type: 'image/png' },
      ]);
      kept(4, ACME.fixLogin, 'Login findings', ada, { bytes: 3 * KIB, version: 2, open: 0 });
      kept(5, ACME.fixLogin, 'Session notes', grace, { bytes: 2 * KIB, version: 1, open: 1 });
      kept(6, ACME.checkout, 'Checkout design', ada, { bytes: 12 * KIB, version: 3, open: 0 });
      kept(7, ACME.payments, 'Fee rules', ada, { bytes: 5 * KIB, version: 1, open: 0 });
      kept(8, ACME.feeTable, 'Fee table findings', ada, { bytes: 4 * KIB, version: 1, open: 0 });
    }
  }
}
