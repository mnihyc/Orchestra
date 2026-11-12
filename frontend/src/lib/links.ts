/** Public addresses and the written references that name the same objects and parts. */
import type { InlineRefKind, NodeKind, ReferencePreviewFile, ReferencePreviewPart } from '@/generated/api-types';
import { contentHref } from '@/features/workbench/contentAddress';
import { publicOrchLink, readOrchLink } from '@/lib/orchLinks';
export { readOrchLink } from '@/lib/orchLinks';

export type LinkKind = InlineRefKind | NodeKind | 'person' | 'group' | 'document_container'
  | 'provider' | 'machine' | 'host' | 'environment' | 'agent' | 'template' | 'mcp_server' | 'mount' | 'skill';

export interface LinkObject {
  kind: LinkKind;
  id: string;
  name?: string;
  title?: string;
  label?: string;
  username?: string;
  node_kind?: NodeKind | null;
  orgId?: string;
  path?: string;
  threadId?: string | null;
  file?: ReferencePreviewFile | null;
  part?: ReferencePreviewPart | null;
}

export interface LinkPart {
  number?: number;
  block?: string;
  comment?: string;
  turn?: number;
  item?: string;
  items?: readonly string[];
  continuation?: string;
  message?: string;
  file?: string;
  line?: number;
  /** An inclusive line selection, while line remains the first line on older screens. */
  endLine?: number;
  path?: string;
  threadId?: string | null;
}

const routes = {
  org: '/org/$id', project: '/project/$id', topic: '/topic/$id', thread: '/thread/$id', document: '/doc/$id', chat: '/chat/$id',
  document_container: '/document-folder/$id', person: '/u/$id', provider: '/resources/providers/$id', machine: '/resources/machines/$id',
  host: '/resources/hosts/$id', environment: '/resources/environments/$id', agent: '/resources/agents/$id',
  template: '/resources/templates/$id', mcp_server: '/resources/mcp/$id', mount: '/resources/mounts/$id', skill: '/resources/skills/$id',
} as const;

function objectPath(kind: keyof typeof routes, id: string): string {
  return routes[kind].replace('$id', encodeURIComponent(id));
}

/** The object of a declared public screen address, using the same route table as Copy link. */
export function linkedObject(href: string): LinkObject | null {
  try {
    const url = readOrchLink(href);
    if (url === null) return null;
    for (const [kind, route] of Object.entries(routes)) {
      const prefix = route.slice(0, route.indexOf('$id'));
      if (!url.pathname.startsWith(prefix)) continue;
      const id = decodeURIComponent(url.pathname.slice(prefix.length).replace(/\/$/, ''));
      if (id === '' || /[/\\]/.test(id) || Array.from(id).some((character) => character.charCodeAt(0) <= 0x1f || character.charCodeAt(0) === 0x7f)) return null;
      return { kind: kind as keyof typeof routes, id };
    }
  } catch { return null; }
  return null;
}

function previewPart(thing: LinkObject): LinkPart {
  const part = thing.part;
  if (part === undefined || part === null) return {};
  const numbered = { number: part.number };
  if (thing.kind === 'document') return { ...numbered, ...(part.anchor === null ? {} : { block: part.anchor }) };
  if (thing.kind === 'chat') return { ...numbered, ...(part.anchor === null ? {} : { message: part.anchor }) };
  if (thing.kind === 'node' || thing.kind === 'thread') return { ...numbered, turn: part.number };
  return { ...numbered, line: part.number };
}

/** A full web address. Existing addressed tabs can also hand on their exact saved address. */
export function linkTo(thing: LinkObject | { href: string }, requested: LinkPart = {}): string {
  if ('href' in thing) {
    const url = new URL(thing.href, location.origin);
    for (const key of ['block', 'comment', 'turn', 'item', 'continuation', 'message', 'file', 'line', 'endLine'] as const) {
      const value = requested[key];
      if (value !== undefined) url.searchParams.set(key, String(value));
    }
    return publicOrchLink(url.href);
  }
  const part = { ...previewPart(thing), ...requested };
  let path: string;
  if (thing.kind === 'environment_file') {
    path = contentHref({ view: 'file', environmentId: thing.id, threadId: part.threadId ?? thing.threadId ?? null,
      path: part.path ?? thing.path ?? thing.file?.path ?? '', ...(part.line === undefined ? {} : { line: part.line }), ...(part.endLine === undefined ? {} : { endLine: part.endLine }) });
    return publicOrchLink(path);
  } else if (thing.kind === 'file') {
    if (thing.file?.owner_kind === 'document' && thing.file.owner_id !== null) {
      path = objectPath('document', thing.file.owner_id);
      part.file = thing.id;
    } else if (thing.file?.owner_id !== undefined && thing.file.owner_id !== null) {
      path = objectPath('chat', thing.file.owner_id);
      part.file = thing.id;
      if (typeof thing.file.message === 'string') part.message = thing.file.message;
    } else {
      // A missing owner has no page to offer; the written reference still names the attachment.
      throw new Error('An attachment link needs its owning document or chat');
    }
  } else if (thing.kind === 'group') {
    path = `/org/${encodeURIComponent(thing.orgId ?? '')}/settings/groups`;
  } else {
    const kind = thing.kind === 'node' ? thing.node_kind ?? 'thread' : thing.kind;
    path = objectPath(kind, kind === 'person' ? thing.username ?? thing.id : thing.id);
  }
  const url = new URL(path, location.origin);
  if (thing.kind === 'group') url.searchParams.set('group', thing.id);
  for (const key of ['block', 'comment', 'turn', 'item', 'continuation', 'message', 'file', 'line', 'endLine'] as const) {
    const value = part[key];
    if (value !== undefined) url.searchParams.set(key, String(value));
  }
  const items = part.items;
  if (items !== undefined) {
    if (part.item === undefined && items[0] !== undefined) url.searchParams.set('item', items[0]);
    if (items.length > 1) url.searchParams.set('items', items.join(','));
  }
  return publicOrchLink(url.href);
}

/** The grammar accepted by agents: one object, an optional workspace path, and a numbered part. */
export function referenceTo(thing: LinkObject | { href: string; name: string }, requested: LinkPart = {}): string {
  if ('href' in thing) {
    const number = requested.number ?? requested.line;
    return markdownReference(thing.name, `${thing.href}${number === undefined ? '' : `#${number}`}`);
  }
  const part = { ...previewPart(thing), ...requested };
  const path = thing.kind === 'environment_file' ? `/${part.path ?? thing.path ?? thing.file?.path ?? ''}` : '';
  const number = part.number ?? part.line ?? part.turn;
  const name = thing.label ?? thing.title ?? thing.name ?? thing.id;
  return markdownReference(name, `id:${thing.id}${path}${number === undefined ? '' : `#${number}`}`);
}

/** Escape labels and destinations without changing their object identity. */
export function markdownReference(name: string, destination: string): string {
  const label = name.replace(/[\\[\]`*_<>$~&|]/g, (character) => `\\${character}`).replace(/\s+/g, ' ');
  const target = /[\s()<>\\]/.test(destination) ? `<${destination.replace(/[<>\\]/g, (character) => `\\${character}`)}>` : destination;
  return `[${label}](${target})`;
}
