/** Uploaded writing imported or copied through the existing document creation producer in the fake world. */
import { createHash, randomUUID } from 'node:crypto';
import type { IncomingMessage } from 'node:http';
import { inflateRawSync } from 'node:zlib';
import type { Document, DocumentRef, NewBlock, DocumentImportResult as ImportResult } from '../../src/generated/api-types.ts';
import { comparePositions } from '../../src/features/documents/blocks.ts';
import { formatOf, IMPORT_ENTRIES_MAX } from '../../src/features/people/imports.ts';
import { readHtml } from '../../src/features/people/html.ts';
import type { Piece } from '../../src/features/documents/pieces.ts';
import { plainText } from '../../src/features/documents/blocks.ts';
import { type Answer, type Person, Refusal, type World } from './world.ts';

/** The real upload contract's chunk size, used by this double's generated upload-session reply. */
const UPLOAD_CHUNK_BYTES = 1024 * 1024;
/** The shipped snapshot bound when a test did not set one at its destination. */
const COPY_BYTES = 64 * 1024 * 1024;
/** A test upload session has the same one-hour life as the ordinary upload service. */
const UPLOAD_LIFE_MS = 60 * 60 * 1000;
/** ZIP's signatures and bounded end-record search, from its fixed record format. */
const ZIP_END = 0x06054b50;
const ZIP_CENTRAL = 0x02014b50;
const ZIP_LOCAL = 0x04034b50;
const ZIP_END_BYTES = 22;
const ZIP_COMMENT_BYTES = 65535;
const clone = <T>(value: T): T => JSON.parse(JSON.stringify(value)) as T;

/** What this double's reader makes of Markdown: headings, bullet items by their indent, and paragraphs. The real reader is the server's; a browser test only needs blocks. */
function readMarkdownLines(source: string): Piece[] {
  const out: Piece[] = [];
  const stack: { indent: number; piece: Piece }[] = [];
  let paragraph: string[] = [];
  const flush = () => {
    if (paragraph.length > 0) out.push({ type: 'paragraph', props: {}, content: [{ type: 'text', text: paragraph.join(' ') }], children: [] });
    paragraph = [];
  };
  for (const line of source.replace(/\r\n?/g, '\n').split('\n')) {
    const heading = /^(#{1,6})\s+(.*?)\s*#*\s*$/.exec(line);
    const item = /^(\s*)[-*+]\s+(.*)$/.exec(line);
    if (heading !== null) {
      flush();
      out.push({ type: 'heading', props: { level: (heading[1] ?? '#').length }, content: [{ type: 'text', text: heading[2] ?? '' }], children: [] });
      stack.length = 0;
    } else if (item !== null) {
      flush();
      const indent = (item[1] ?? '').length;
      const made: Piece = { type: 'bullet', props: {}, content: [{ type: 'text', text: item[2] ?? '' }], children: [] };
      while (stack.length > 0 && (stack[stack.length - 1]?.indent ?? 0) >= indent) stack.pop();
      (stack[stack.length - 1]?.piece.children ?? out).push(made);
      stack.push({ indent, piece: made });
    } else if (line.trim() === '') {
      flush();
    } else {
      paragraph.push(line.trim());
    }
  }
  flush();
  return out;
}

/** A title supplied by the first heading is stored once, outside the copied block tree, and every block has a fresh id: what the server's import makes, in the small. */
function importedWriting(name: string, source: string, format: 'markdown' | 'html', freshId: () => string): { title: string; blocks: NewBlock[]; warnings: string[] } {
  const html = format === 'html' ? readHtml(source) : null;
  const pieces = [...(html?.pieces ?? readMarkdownLines(source))];
  const first = pieces[0];
  const heading = first?.type === 'heading' ? plainText(first.content).trim() : '';
  const title = (heading || name.replace(/^.*\//, '').replace(/\.[^.]+$/, '')).slice(0, 200) || 'Untitled';
  if (heading !== '' && heading === title) pieces.shift();
  const copy = (one: Piece): NewBlock => ({ id: freshId(), type: one.type, props: one.props, content: one.content, children: one.children.map(copy) });
  return { title, blocks: pieces.map(copy), warnings: html?.warnings ?? [] };
}

interface Upload { name: string; size: number; media_type: string; bytes: Buffer; owner: string }
function entriesOfZip(data: Buffer, max: number): { name: string; data: Buffer | null; error: string | null }[] {
  let end = data.length - ZIP_END_BYTES;
  const earliest = Math.max(0, end - ZIP_COMMENT_BYTES);
  while (end >= earliest && data.readUInt32LE(end) !== ZIP_END) end -= 1;
  if (end < earliest) throw new Error('Not a supported ZIP archive');
  const count = data.readUInt16LE(end + 10);
  if (count > IMPORT_ENTRIES_MAX) throw new Error('The archive has more than 200 entries');
  let cursor = data.readUInt32LE(end + 16); let expanded = 0;
  const rows: { name: string; data: Buffer | null; error: string | null }[] = [];
  for (let index = 0; index < count; index += 1) {
    if (cursor + 46 > data.length || data.readUInt32LE(cursor) !== ZIP_CENTRAL) throw new Error('The archive directory is damaged');
    const flags = data.readUInt16LE(cursor + 8); const method = data.readUInt16LE(cursor + 10);
    const compressed = data.readUInt32LE(cursor + 20); const size = data.readUInt32LE(cursor + 24);
    const nameBytes = data.readUInt16LE(cursor + 28); const extra = data.readUInt16LE(cursor + 30); const comment = data.readUInt16LE(cursor + 32);
    const mode = data.readUInt32LE(cursor + 38) >>> 16; const local = data.readUInt32LE(cursor + 42);
    const name = data.subarray(cursor + 46, cursor + 46 + nameBytes).toString('utf8'); cursor += 46 + nameBytes + extra + comment;
    if (name.endsWith('/')) continue;
    const unsafe = /^[/\\]|^[A-Za-z]:|[\\]/.test(name) || name.split('/').includes('..') || (mode & 0xf000) === 0xa000 || name.includes('\0');
    if (unsafe) { rows.push({ name, data: null, error: 'Unsafe archive path or link' }); continue; }
    if ((flags & 1) !== 0 || ![0, 8].includes(method)) { rows.push({ name, data: null, error: 'Encrypted or unsupported archive entry' }); continue; }
    expanded += size;
    if (expanded > max) throw new Error('The expanded archive exceeds this destination’s copy limit');
    if (local + 30 > data.length || data.readUInt32LE(local) !== ZIP_LOCAL) throw new Error('An archive entry is damaged');
    const start = local + 30 + data.readUInt16LE(local + 26) + data.readUInt16LE(local + 28);
    if (start + compressed > data.length) throw new Error('An archive entry is truncated');
    const packed = data.subarray(start, start + compressed);
    const unpacked = method === 0 ? packed : inflateRawSync(packed, { maxOutputLength: Math.max(1, Math.min(size, max)) });
    if (unpacked.length !== size) throw new Error('An archive entry has the wrong expanded size');
    rows.push({ name, data: unpacked, error: null });
  }
  return rows;
}

export class PeopleImports {
  readonly world: World;
  sessions = new Map<string, Upload>();
  blobs = new Map<string, Upload>();
  copied = new Map<string, Document>();
  made = new Map<string, { document: DocumentRef; warnings: string[] }>();
  constructor(world: World) { this.world = world; }
  private guard(person: Person, nodeId: string): number {
    this.world.places.need(person, nodeId, 'doc.create');
    const settings = this.world.settings.handle(person, 'GET', `/nodes/${nodeId}/settings`, new URLSearchParams(), {})?.body as { key: string; value: unknown }[] | undefined;
    if (settings?.find((row) => row.key === 'docs.enabled')?.value === false) throw new Refusal(403, 'docs.disabled', 'Documents are off here');
    const max = settings?.find((row) => row.key === 'sharing.snapshot_max_bytes')?.value;
    return typeof max === 'number' ? max : COPY_BYTES;
  }
  async raw(person: Person, request: IncomingMessage, method: string, path: string, query: URLSearchParams): Promise<Answer | null> {
    const match = /^\/uploads\/sessions\/([^/]+)$/.exec(path);
    if (method !== 'PUT' || match === null) return null;
    const upload = this.sessions.get(match[1] ?? '');
    if (upload?.owner !== person.me.id) throw new Refusal(404, 'not_found', 'No such upload');
    if (Number(query.get('offset')) !== upload.bytes.length) throw new Refusal(409, 'conflict.stale_revision', 'Upload offset changed', { received: upload.bytes.length });
    const chunks: Buffer[] = []; let bytes = 0;
    for await (const chunk of request) { const part = Buffer.from(chunk as Uint8Array); bytes += part.length; if (bytes > UPLOAD_CHUNK_BYTES || upload.bytes.length + bytes > upload.size) throw new Refusal(413, 'docs.too_large', 'Upload piece is too large'); chunks.push(part); }
    upload.bytes = Buffer.concat([upload.bytes, ...chunks]);
    return { status: 200, body: { received: upload.bytes.length } };
  }
  handle(person: Person, method: string, path: string, query: URLSearchParams, body: Record<string, unknown>): Answer | null {
    if (method === 'POST' && path === '/uploads/sessions') {
      const id = randomUUID(); this.sessions.set(id, { name: String(body.name), size: Number(body.size), media_type: String(body.media_type), bytes: Buffer.alloc(0), owner: person.me.id });
      return { status: 201, body: { upload_id: id, chunk_bytes: UPLOAD_CHUNK_BYTES, expires_at: new Date(Date.now() + UPLOAD_LIFE_MS).toISOString() } };
    }
    let match: RegExpExecArray | null;
    if (method === 'POST' && (match = /^\/uploads\/sessions\/([^/]+)\/complete$/.exec(path))) {
      const upload = this.sessions.get(match[1] ?? '');
      if (upload?.owner !== person.me.id || upload.bytes.length !== upload.size) throw new Refusal(400, 'request.invalid', 'The upload is incomplete');
      // The double needs a stable content identity; only the real blob store promises its hash algorithm.
      const blob = createHash('sha256').update(upload.bytes).digest('hex'); this.blobs.set(blob, upload);
      return { status: 200, body: { blob, name: upload.name, size: upload.size, media_type: upload.media_type } };
    }
    if (method === 'POST' && (match = /^\/nodes\/([^/]+)\/documents\/copy$/.exec(path))) {
      const destination = match[1] ?? ''; const max = this.guard(person, destination);
      const stable = `${person.me.id}:${destination}:${String(body.client_id)}`;
      const previous = this.copied.get(stable); if (previous !== undefined) return { status: 201, body: previous };
      const source = this.world.documents.readable(person).find((doc) => doc.id === body.source_document_id);
      if (source === undefined) throw new Refusal(404, 'not_found', 'No readable source document');
      if (Buffer.byteLength(JSON.stringify([...source.blocks.values()]),'utf8') + source.files.reduce((bytes,file) => bytes+file.size,0) > max) throw new Refusal(413,'docs.too_large','The copy exceeds this destination’s snapshot limit');
      const ids = new Map([...source.blocks.keys(), ...source.files.map((file) => file.id)].map((id) => [id, randomUUID()]));
      const rewrite = (value: unknown, to: string): unknown => {
        if (Array.isArray(value)) return value.map((item) => rewrite(item,to));
        if (value === null || typeof value !== 'object') return value;
        const row: Record<string,unknown> = Object.fromEntries(Object.entries(value).map(([key,item]) => [key,rewrite(item,to)]));
        if (row.type === 'ref' && row.kind === 'document' && row.id === source.id) row.id=to;
        const href = row.href;
        if (typeof href === 'string') {
          const own = href.split(/[?#]/,1)[0] === `/doc/${source.id}`;
          if (own || /^[?#]/.test(href)) {
            const rewritten = href.replace(/([?&#]block=)([^&#]+)/g,(full: string,prefix: string,old: string) => ids.has(old) ? `${prefix}${ids.get(old) ?? old}` : full);
            row.href = own ? `/doc/${to}${rewritten.slice(`/doc/${source.id}`.length)}` : rewritten;
          }
        }
        return row;
      };
      const children = (parent: string | null): NewBlock[] => [...source.blocks.values()].filter((block) => !block.removed && block.parent_id === parent).sort((a, b) => comparePositions(a.position, b.position)).map((block) => {
        const file = 'file_id' in block.props && typeof block.props.file_id === 'string' ? block.props.file_id : null;
        return { id:ids.get(block.id) ?? randomUUID(),type:block.type,props:{...block.props,...(file === null ? {} : {file_id:ids.get(file) ?? file})},content:clone(block.content),children:children(block.id) };
      });
      const answer = this.world.documents.handle(person, 'POST', `/nodes/${destination}/documents`, query, { title: typeof body.title === 'string' ? body.title : source.title, blocks: children(null), access: body.access });
      if (answer === null) return null;
      const made = answer.body as Document;
      const stored = this.world.documents.docs.get(made.id);
      if (stored !== undefined) {
        stored.files = source.files.map((file) => ({ ...clone(file), id: ids.get(file.id) ?? randomUUID(), added_by: made.created_by, added_at: made.created_at }));
        for (const block of stored.blocks.values()) block.content=rewrite(block.content,made.id) as NewBlock['content'] ?? [];
      }
      const current = this.world.documents.handle(person,'GET',`/documents/${made.id}`,query,{})?.body as Document | undefined;
      if (current === undefined) throw new Refusal(404,'not_found','The new document cannot be read');
      this.copied.set(stable,current); return { status:201, body:current };
    }
    if (method === 'POST' && (match = /^\/nodes\/([^/]+)\/documents\/import$/.exec(path))) {
      const destination = match[1] ?? ''; const max = this.guard(person, destination);
      const files = Array.isArray(body.files) ? body.files as { blob: string; name: string; format: string }[] : [];
      if (files.length === 0 || files.length > IMPORT_ENTRIES_MAX || (files.length > 1 && files.some((file) => file.format === 'archive'))) throw new Refusal(400, 'request.invalid', 'Choose writing files or one archive');
      const result: ImportResult = { entries: [] }; const usedNames = new Set<string>();
      for (const file of files) {
        const stored = this.blobs.get(file.blob);
        if (stored?.owner !== person.me.id) throw new Refusal(404, 'not_found', 'No uploaded file');
        let entries: { name: string; data: Buffer | null; error: string | null }[];
        try { entries = file.format === 'archive' ? entriesOfZip(stored.bytes, max) : [{ name: file.name, data: stored.bytes, error: null }]; }
        catch (failure) { result.entries.push({ name: file.name, document: null, warnings: [], error: { code: 'request.invalid', message: String(failure) } }); continue; }
        for (const entry of entries) {
          const stable = `${person.me.id}:${destination}:${String(body.client_id)}:${entry.name}`;
          if (usedNames.has(entry.name)) { result.entries.push({ name: entry.name, document: null, warnings: [], error: { code: 'request.invalid', message: 'Duplicate entry' } }); continue; }
          usedNames.add(entry.name);
          const previous = this.made.get(stable);
          if (previous !== undefined) { result.entries.push({ name: entry.name, document: previous.document, warnings: previous.warnings, error: null }); continue; }
          const format = formatOf(entry.name);
          if (entry.error !== null || entry.data === null || (format !== 'markdown' && format !== 'html')) { result.entries.push({ name: entry.name, document: null, warnings: [], error: { code: 'request.invalid', message: entry.error ?? 'Unsupported entry' } }); continue; }
          try {
            this.guard(person, destination);
            const converted = importedWriting(entry.name, new TextDecoder('utf-8', { fatal: true }).decode(entry.data), format, randomUUID);
            const answer = this.world.documents.handle(person, 'POST', `/nodes/${destination}/documents`, query, converted);
            const doc = answer?.body as Document | undefined;
            if (doc === undefined) throw new Error('Creation unavailable');
            this.made.set(stable, { document: doc, warnings: converted.warnings }); result.entries.push({ name: entry.name, document: doc, warnings: converted.warnings, error: null });
          } catch (failure) { result.entries.push({ name: entry.name, document: null, warnings: [], error: { code: 'request.invalid', message: String(failure) } }); }
        }
      }
      return { status: 200, body: result };
    }
    return null;
  }
}
