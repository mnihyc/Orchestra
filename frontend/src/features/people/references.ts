/** References a person meets in Orch's text: a chip's typed reference read from its address, where it opens, and the words that hand it to an agent.
 *
 * A reference names a thing (a place, a thread, a document, a chat) and may name one of its parts: a block of a
 * document by its identifier, a turn or a message by its number. In a document it is a chip of its own; in the text
 * of a message it is a link to its address, `/documents/<id>#block=<id>`, `/nodes/<id>#turn=4`,
 * `/chats/<id>#message=12`. Copied for an agent it is the grammar agents write, `[name](id:<id>#n)`, which any
 * agent that may read the thing follows with `read_shared`.
 */
import { placePath } from '@/features/navigation/places';
import { contentHref } from '@/features/workbench/contentAddress';
import type { InlineRefKind, NodeKind, ReferencePreview } from '@/generated/api-types';

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
// These are stored reference segments, not destinations in the browser's route tree.
const BASES = new Map<string, InlineRefKind>([
  ['nodes', 'node'],
  ['documents', 'document'],
  ['chats', 'chat'],
  ['files', 'file'],
]);

/** A typed reference read from the address a message keeps it as; null for any other address. */
export function referenceOfAddress(href: string): TypedReference | null {
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
  const kind = BASES.get(address?.[1] ?? '');
  const id = address?.[2];
  if (kind === undefined || id === undefined || !UUID.test(id)) return null;
  const reference: TypedReference = { kind, id };
  const [key, value = ''] = fragment.split('=', 2);
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
  const part = preview.part;
  const anchor = part?.anchor ?? null;
  switch (preview.kind) {
    case 'environment_file':
      return contentHref({ view: 'file', environmentId: preview.id, threadId: null, path: preview.file?.path ?? '', ...(part === null ? {} : { line: part.number }) });
    case 'file': {
      const file = preview.file;
      if (file?.owner_kind === 'document' && file.owner_id !== null) return `/doc/${file.owner_id}?file=${preview.id}${part === null ? '' : `&line=${part.number}`}`;
      const message = file?.message;
      return `/chat/${file?.owner_id ?? ''}${message === undefined || message === null ? '' : `?message=${encodeURIComponent(message)}`}`;
    }
    case 'document':
      return anchor === null ? `/doc/${preview.id}` : `/doc/${preview.id}?block=${encodeURIComponent(anchor)}`;
    case 'chat':
      return anchor === null ? `/chat/${preview.id}` : `/chat/${preview.id}?message=${encodeURIComponent(anchor)}`;
    default: {
      const kind: NodeKind = preview.node_kind ?? 'thread';
      const path = placePath({ id: preview.id, kind });
      return part === null || kind !== 'thread' ? path : `${path}?turn=${part.number}`;
    }
  }
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
  const part = preview.part === null ? '' : `#${preview.part.number}`;
  const path = preview.kind === 'environment_file' ? `/${preview.file?.path ?? ''}` : '';
  const destination = `id:${preview.id}${path}${part}`;
  return markdownReference(preview.title, destination);
}

/** A file copied for an agent, in the same grammar a card writes. */
export function environmentFileReference(environment: string, path: string, line?: number): string {
  const name = `${path.split('/').at(-1) ?? path}${line === undefined ? '' : `:${line}`}`;
  const destination = `id:${environment}/${path}${line === undefined ? '' : `#${line}`}`;
  return markdownReference(name, destination);
}

/** An attachment copied for an agent, optionally at the first selected text line. */
export function attachmentFileReference(id: string, name: string, line?: number): string {
  return idReferenceText(id, `${name}${line === undefined ? '' : `:${line}`}`, line);
}

/** Names a stable object and its numbered part in the shared written grammar. */
export function idReferenceText(id: string, name: string, part?: number): string {
  return markdownReference(name, `id:${id}${part === undefined ? '' : `#${part}`}`);
}

/** Writes the same link grammar for a saved reference and for a file's receiving path. */
export function markdownReference(name: string, destination: string): string {
  const label = name.replace(/[\\[\]`*_<>$~&|]/g, (character) => `\\${character}`).replace(/\s+/g, ' ');
  const target = /[\s()<>\\]/.test(destination) ? `<${destination.replace(/[<>\\]/g, (character) => `\\${character}`)}>` : destination;
  return `[${label}](${target})`;
}
