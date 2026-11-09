/** References a person meets in Orch's text: a chip's typed reference read from its address, where it opens, and the words that hand it to an agent.
 *
 * A reference names a thing (a place, a thread, a document, a chat) and may name one of its parts: a block of a
 * document by its identifier, a turn or a message by its number. In a document it is a chip of its own; in the text
 * of a message it is a link to its address, `/documents/<id>#block=<id>`, `/nodes/<id>#turn=4`,
 * `/chats/<id>#message=12`. Copied for an agent it is the grammar agents write, `[name](id:<id>#n)`, which any
 * agent that may read the thing follows with `read_shared`.
 */
import { parseContentAddress } from '@/features/workbench/contentAddress';
import { linkTo, referenceTo } from '@/lib/links';
import { readOrchLink } from '@/lib/orchLinks';
export { markdownReference } from '@/lib/links';
import type { InlineRefKind, ReferencePreview } from '@/generated/api-types';

/** What a chip names: a thing, and the part of it. */
export interface TypedReference {
  kind: InlineRefKind;
  id: string;
  block?: string;
  turn?: number;
  message?: number;
  path?: string;
  line?: number;
  /** The name written with the reference, retained even when the reader cannot open it. */
  label?: string;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
// Written reference segments and the public screen addresses of the same things.
const BASES = new Map<string, InlineRefKind>([
  ['nodes', 'node'],
  ['documents', 'document'],
  ['chats', 'chat'],
  ['files', 'file'],
  ['org', 'node'], ['project', 'node'], ['topic', 'node'], ['thread', 'node'],
  ['doc', 'document'], ['chat', 'chat'],
]);

/** A typed reference read from the address a message keeps it as; null for any other address. */
export function referenceOfAddress(href: string): TypedReference | null {
  const url = readOrchLink(href);
  if (url === null) return null;
  href = `${url.pathname}${url.search}${url.hash}`;
  const content = parseContentAddress(href);
  if (content?.view === 'file' && UUID.test(content.environmentId)) return { kind: 'environment_file', id: content.environmentId, path: content.path, ...(content.line === undefined ? {} : { line: content.line }) };
  const [head = '', fragment = ''] = href.split('#', 2);
  const path = (head.split('?')[0] ?? '').replace(/\/+$/, '');
  const environment = /^\/environments\/([^/]+)\/files\/(.+)$/.exec(path);
  if (environment?.[1] !== undefined && environment[2] !== undefined && UUID.test(environment[1])) {
    try {
      const filePath = decodeURIComponent(environment[2]);
      if (filePath.startsWith('/') || filePath.split('/').some((part) => part === '..' || part === '.' || part === '')) return null;
      const line = /^line=([1-9]\d*)$/.exec(fragment)?.[1];
      return { kind: 'environment_file', id: environment[1], path: filePath, ...(line === undefined ? {} : { line: Number(line) }) };
    } catch { return null; }
  }
  const address = /^\/([^/]+)\/([^/]+)$/.exec(path);
  const route = address?.[1] ?? '';
  const kind = BASES.get(route);
  let id: string | undefined;
  try { id = address?.[2] === undefined ? undefined : decodeURIComponent(address[2]); } catch { return null; }
  if (kind === undefined || id === undefined || !UUID.test(id)) return null;
  const reference: TypedReference = { kind, id };
  const query = new URLSearchParams(head.split('?')[1] ?? '');
  const file = query.get('file'), line = query.get('line');
  if ((route === 'doc' || route === 'chat') && file !== null && UUID.test(file)) return {
    kind: 'file', id: file, ...(line !== null && /^[1-9]\d*$/.test(line) ? { line: Number(line) } : {}),
  };
  const [key, value = ''] = fragment.split('=', 2);
  const block = query.get('block');
  if (kind === 'document' && block !== null && UUID.test(block)) reference.block = block;
  const turn = query.get('turn');
  if (kind === 'node' && turn !== null && /^[1-9]\d*$/.test(turn)) reference.turn = Number(turn);
  const message = query.get('message');
  if (kind === 'chat' && message !== null && /^[1-9]\d*$/.test(message)) reference.message = Number(message);
  const number = /^[1-9]\d*$/.test(value) ? Number(value) : undefined;
  if (kind === 'document' && key === 'block' && UUID.test(value)) reference.block = value;
  if (kind === 'node' && key === 'turn' && number !== undefined) reference.turn = number;
  if (kind === 'chat' && key === 'message' && number !== undefined) reference.message = number;
  if (kind === 'file' && key === 'line' && number !== undefined) reference.line = number;
  return reference;
}

/** Where a reference opens in the application, at its part: a document at its block, a thread at its turn, a chat
 * at its message. */
export function referenceTarget(preview: ReferencePreview): string {
  if (preview.kind === 'file' && (preview.file?.owner_id === null || preview.file?.owner_id === undefined)) return '';
  const url = new URL(linkTo(preview));
  return `${url.pathname}${url.search}${url.hash}`;
}

/** A reference as an agent writes it, `id:<id>` with the number of a part after `#` (a document's top-level block, a
 * turn, a message, as `read_shared` counts them): the thing, whose kind it does not say, and the number. Null for any
 * other text. */
export function idReferenceOf(text: string): { id: string; path?: string; number?: number } | null {
  const found = /^id:([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:\/(.+?))?(?:#([1-9]\d*))?$/i.exec(text.trim());
  if (found?.[1] === undefined) return null;
  return { id: found[1], ...(found[2] === undefined ? {} : { path: found[2] }), ...(found[3] === undefined ? {} : { number: Number(found[3]) }) };
}

/** The reference as an agent writes and follows it: `[name](id:<id>#n)`, its words escaped as Markdown wants. */
export function referenceText(preview: ReferencePreview): string {
  return referenceTo(preview);
}

/** A file copied for an agent, in the same grammar a card writes. */
export function environmentFileReference(environment: string, path: string, line?: number): string {
  const name = `${path.split('/').at(-1) ?? path}${line === undefined ? '' : `:${line}`}`;
  return referenceTo({ kind: 'environment_file', id: environment, path, name }, line === undefined ? {} : { line });
}

/** An attachment copied for an agent, optionally at the first selected text line. */
export function attachmentFileReference(id: string, name: string, line?: number): string {
  return referenceTo({ kind: 'file', id, name: `${name}${line === undefined ? '' : `:${line}`}` }, line === undefined ? {} : { line });
}

/** Names a stable object and its numbered part in the shared written grammar. */
export function idReferenceText(id: string, name: string, part?: number): string {
  return referenceTo({ kind: 'node', id, name }, part === undefined ? {} : { number: part });
}
