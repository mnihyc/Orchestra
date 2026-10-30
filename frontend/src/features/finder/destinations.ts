/** Where a name leads, and how a result says what it is and where it lies.
 *
 * A destination is a route as the router names it (`/thread/$threadId`) with the values of its
 * segments and its search, so that the same description opens the screen here, writes the address of a
 * new browser tab. Commands describe routes; name hits carry the server's finished address. A name
 * without an address still opens its card or project chooser.
 */
import type { TFunction } from 'i18next';
import { Bot, Boxes, Cpu, GraduationCap, HardDrive, Laptop, type LucideIcon, MessagesSquare, PlugZap, Server, Tags, UserRound, UsersRound, Wrench } from 'lucide-react';

import type { NameHit, NameKind, NodeRef } from '@/generated/api-types';
import { NODE_GLYPHS, type NodeIconKind } from '@/ui/NodeIcon';
import { labelsAlongPath } from '@/features/search/labels';

/** A route, the values of the `$names` in it, and the search of the address. */
export interface Destination {
  to: string;
  params?: Record<string, string>;
  search?: Record<string, string | undefined>;
}

/** The address a destination writes: segments filled in and escaped, search written after a question mark. */
export function pathOf({ to, params = {}, search = {} }: Destination): string {
  const path = to.replace(/\$([A-Za-z]+)/g, (_, name: string) => encodeURIComponent(params[name] ?? ''));
  const hashAt = path.indexOf('#');
  const hash = hashAt < 0 ? '' : path.slice(hashAt);
  const withoutHash = hashAt < 0 ? path : path.slice(0, hashAt);
  const questionAt = withoutHash.indexOf('?');
  const base = questionAt < 0 ? withoutHash : withoutHash.slice(0, questionAt);
  const query = new URLSearchParams(questionAt < 0 ? '' : withoutHash.slice(questionAt + 1));
  for (const [key, value] of Object.entries(search)) if (value !== undefined && value !== '') query.set(key, value);
  const text = query.toString();
  return `${base}${text === '' ? '' : `?${text}`}${hash}`;
}

/** What separates the places of a way: "Web › Checkout". */
export const WAY_SEPARATOR = ' › ';

/** The places a name lies in, from the root org down, without the thing itself: the way a path is shown. */
export function ancestorsOf(hit: Pick<NameHit, 'id' | 'path'>): NodeRef[] {
  const way = hit.path;
  return way.at(-1)?.id === hit.id ? way.slice(0, -1) : [...way];
}

/** The way to a name as words: "Acme › Web". A place the reader may not know the name of is told as "Private project". */
export function wayOf(hit: Pick<NameHit, 'id' | 'path'>, t: TFunction, labels: ReadonlyMap<string, string> = new Map()): string {
  return labelsAlongPath(ancestorsOf(hit), labels, (kind) => t('finder.private', { kind: t(`finder.kinds.${kind}`) }))
    .join(WAY_SEPARATOR);
}

/** The kind of thing a name is, as the finder says it: a project that lies in a project is a folder, an org in an org a department. */
export type Described = NameKind | 'folder' | 'department';

export function describe(hit: Pick<NameHit, 'id' | 'kind' | 'path'>): Described {
  const above = ancestorsOf(hit).at(-1);
  if (hit.kind === 'project' && above?.kind === 'project') return 'folder';
  if (hit.kind === 'org' && above !== undefined) return 'department';
  return hit.kind;
}

/** The icon of each kind of thing; a place has the glyph it has everywhere. */
export const KIND_ICONS: Record<Described, LucideIcon> = {
  org: NODE_GLYPHS.org,
  department: NODE_GLYPHS.org,
  project: NODE_GLYPHS.project,
  folder: NODE_GLYPHS.folder,
  topic: NODE_GLYPHS.topic,
  thread: NODE_GLYPHS.thread,
  document: NODE_GLYPHS.document,
  document_container: NODE_GLYPHS.folder,
  chat: MessagesSquare,
  tag: Tags,
  user: UserRound,
  group: UsersRound,
  provider: PlugZap,
  agent: Bot,
  environment: Boxes,
  host: Server,
  template: Cpu,
  mcp_server: Wrench,
  mount: HardDrive,
  machine: Laptop,
  skill: GraduationCap,
};

export function iconOf(kind: Described): LucideIcon {
  return KIND_ICONS[kind];
}

/** What kind of place a thing is, for its icon; `null` for what is no place (a person, a chat, a resource). */
export function placeKindOf(kind: Described): NodeIconKind | null {
  switch (kind) {
    case 'org':
    case 'department':
      return 'org';
    case 'project':
    case 'folder':
    case 'topic':
    case 'thread':
    case 'document':
      return kind;
    default:
      return null;
  }
}

/** The username of a person found by address: the address ends with it. */
export function usernameOf(hit: Pick<NameHit, 'address'> & Partial<Pick<NameHit, 'username'>>): string | null {
  return hit.username ?? hit.address?.split('.').at(-1) ?? null;
}

/** Names open the one address served by the API; the current screen never chooses their org or route. */
export function destinationOf(hit: Pick<NameHit, 'href'>): Destination | null {
  return hit.href === undefined || hit.href === null ? null : { to: hit.href };
}
