/** Pure shared addresses for full content. Local view identities never enter a resource URL. */
import { defaultParseSearch, defaultStringifySearch } from '@tanstack/react-router';
import type { InspectorTarget, Tab } from './layout';
import { locatePage, pageParam, type PageTarget } from './pageTargets';

export const EMPTY_WORKBENCH = '/workbench';
export interface WorkspaceSource { environmentId: string; threadId: string | null }
export type ContentLocation =
  | { view: 'page'; href: string }
  | ({ view: 'file'; path: string; line?: number } & WorkspaceSource)
  | ({ view: 'difference'; path: string; kind: 'git' | 'change'; item: string; staged: boolean } & WorkspaceSource);

export function relativeFilePath(path: string): boolean {
  return path !== '' && !path.startsWith('/') && !path.includes('\\') && !path.includes('\0')
    && path.split('/').every((part) => part !== '' && part !== '.' && part !== '..');
}
export function fileLine(value: unknown): number | undefined {
  if (typeof value !== 'string' && typeof value !== 'number') return undefined;
  if (!/^\d+$/.test(String(value))) return undefined;
  const line = Number(value);
  return Number.isSafeInteger(line) && line > 0 ? line : undefined;
}
function internalAddress(href: string): URL | null {
  if (!href.startsWith('/') || href.startsWith('//') || href.includes('\\')) return null;
  try { return new URL(href, 'https://orch.invalid'); } catch { return null; }
}
export function parsePageAddress(href: string): PageTarget | null {
  const url = internalAddress(href);
  if (url === null) return null;
  const located = locatePage(url.pathname);
  return located === null ? null : { ...located, search: defaultParseSearch(url.search), hash: url.hash.slice(1) };
}
export function pageHref(target: PageTarget): string {
  return `${target.pathname}${defaultStringifySearch(target.search)}${target.hash === '' ? '' : `#${target.hash}`}`;
}
export function pageResource(target: PageTarget | null): InspectorTarget | null {
  if (target === null) return null;
  const { route } = target;
  if (route === '/thread/$threadId/') return { kind: 'thread', id: pageParam(target, 'threadId') };
  if (route === '/doc/$documentId/') return { kind: 'document', id: pageParam(target, 'documentId') };
  if (route === '/project/$projectId/') return { kind: 'project', id: pageParam(target, 'projectId') };
  if (route === '/topic/$topicId/') return { kind: 'topic', id: pageParam(target, 'topicId') };
  if (route === '/resources/environments/$id/') return { kind: 'environment', id: pageParam(target, 'id') };
  return null;
}

/** Reject malformed/ambiguous resource selectors, rather than opening a different source. */
export function parseContentAddress(href: string): ContentLocation | null {
  const url = internalAddress(href);
  if (url === null) return null;
  if (url.pathname.replace(/\/$/, '') !== EMPTY_WORKBENCH) {
    const target = parsePageAddress(href);
    return { view: 'page', href: target === null ? url.pathname + url.search + url.hash : pageHref(target) };
  }
  const query = url.searchParams;
  for (const key of ['view', 'environment', 'thread', 'path', 'line', 'kind', 'item', 'staged']) {
    if (query.getAll(key).length > 1) return null;
  }
  const values: Record<string, unknown> = defaultParseSearch(url.search);
  // The application router quotes JSON-looking strings. Accept raw old links too, while keeping
  // filenames such as 123, true and [draft] as strings instead of converting their identity.
  const scalar = (key: string): string | null => {
    if (!query.has(key)) return null;
    const value = Object.hasOwn(values, key) ? values[key] : undefined;
    return typeof value === 'string' ? value : query.get(key);
  };
  const view = scalar('view'), environmentId = scalar('environment'), path = scalar('path');
  const through = scalar('thread');
  if ((view !== 'file' && view !== 'difference') || environmentId === null || environmentId === ''
    || path === null || !relativeFilePath(path) || through === '' || url.hash !== '') return null;
  const source = { environmentId, threadId: through };
  if (view === 'file') {
    if (query.has('kind') || query.has('item') || query.has('staged')) return null;
    const line = fileLine(scalar('line'));
    if (query.has('line') && line === undefined) return null;
    return { view, ...source, path, ...(line === undefined ? {} : { line }) };
  }
  if (query.has('line')) return null;
  const kind = scalar('kind'), item = scalar('item') ?? '', staged = scalar('staged');
  if (kind !== 'git' && kind !== 'change' || staged !== null && staged !== '0' && staged !== '1') return null;
  if (kind === 'git' && item !== '' || kind === 'change' && staged !== null) return null;
  return { view, ...source, path, kind, item, staged: kind === 'git' && staged === '1' };
}

export function contentHref(location: ContentLocation): string {
  if (location.view === 'page') return location.href;
  const query: Record<string, string | number> = { view: location.view, environment: location.environmentId };
  if (location.threadId !== null) query.thread = location.threadId;
  query.path = location.path;
  if (location.view === 'file') {
    if (location.line !== undefined) query.line = location.line;
  } else {
    query.kind = location.kind;
    if (location.kind === 'git') query.staged = location.staged ? 1 : 0;
    else if (location.item !== '') query.item = location.item;
  }
  return `${EMPTY_WORKBENCH}${defaultStringifySearch(query)}`;
}

/** Clone resource data only; the store supplies a fresh local owner for new/beside. */
export function contentTab(location: ContentLocation): Pick<Tab, 'view' | 'of'> {
  if (location.view === 'page') return { view: 'page', of: { href: location.href } };
  const source = { env: location.environmentId, thread: location.threadId ?? '', path: location.path };
  return location.view === 'file' ? { view: 'file', of: { ...source, ...(location.line === undefined ? {} : { line: String(location.line) }) } }
    : { view: 'difference', of: { ...source, kind: location.kind, item: location.item, staged: location.staged ? '1' : '0' } };
}
export function contentLocation(tab: Tab): ContentLocation | null {
  const of = tab.of;
  if (tab.view === 'page') return of?.href === undefined ? null : { view: 'page', href: of.href };
  if ((tab.view !== 'file' && tab.view !== 'difference') || of?.env === undefined || of.env === ''
    || of.path === undefined || !relativeFilePath(of.path) || of.thread === undefined) return null;
  const source = { environmentId: of.env, threadId: of.thread === '' ? null : of.thread, path: of.path };
  if (tab.view === 'file') {
    const line = fileLine(of.line);
    return { view: 'file', ...source, ...(line === undefined ? {} : { line }) };
  }
  return { view: 'difference', ...source, kind: of.kind === 'git' ? 'git' : 'change', item: of.item ?? '', staged: of.kind === 'git' && of.staged === '1' };
}
/** Native thread of full/legacy conversation content; no local owner is interpreted as an ID. */
export function contentThreadId(tab: Tab): string | null {
  if (tab.view === 'search-result' && tab.of?.kind === 'thread') return tab.of.id ?? null;
  const location = contentLocation(tab);
  const target = location?.view === 'page' ? pageResource(parsePageAddress(location.href)) : null;
  return target?.kind === 'thread' ? target.id : null;
}
export function contentAddress(tab: Tab): string | null {
  const location = contentLocation(tab);
  return location === null ? null : contentHref(location);
}
export function sameContentResource(one: ContentLocation, other: ContentLocation): boolean {
  if (one.view === 'page') return other.view === 'page'
    && (parsePageAddress(one.href)?.pathname ?? one.href.split(/[?#]/)[0]) === (parsePageAddress(other.href)?.pathname ?? other.href.split(/[?#]/)[0]);
  if (other.view === 'page' || one.view !== other.view || one.environmentId !== other.environmentId
    || one.threadId !== other.threadId || one.path !== other.path) return false;
  if (one.view === 'file') return other.view === 'file';
  return other.view === 'difference' && one.kind === other.kind && one.item === other.item && one.staged === other.staged;
}
export function contentTarget(tab: Tab): InspectorTarget | null {
  const location = contentLocation(tab);
  if (location === null) return null;
  if (location.view === 'page') return pageResource(parsePageAddress(location.href));
  return { kind: 'environment', id: location.environmentId,
    workspace: { environmentId: location.environmentId, threadId: location.threadId } };
}

/** Supported old main results keep their exact owner and locator when promoted to a full page. */
export function migrateMainResult(tab: Tab): Tab {
  const of = tab.of;
  if (tab.view !== 'search-result' || of?.id === undefined || of.id === '') return tab;
  const route = of.kind === 'thread' ? 'thread' : of.kind === 'document' ? 'doc' : of.kind === 'chat' ? 'chat' : of.kind === 'place-chat' ? 'place-chat' : null;
  if (route === null) return tab;
  const path = `/${route}/${encodeURIComponent(of.id)}`;
  const previous = of.href === undefined ? null : internalAddress(of.href);
  const url = new URL(path, 'https://orch.invalid');
  if (previous?.pathname.replace(/\/$/, '') === path) { url.search = previous.search; url.hash = previous.hash; }
  // Older chat entrances used their place URL; only a matching explicit source may supply its anchor.
  if (previous !== null && of.anchor === undefined) {
    const pieces = previous.pathname.replace(/\/$/, '').split('/');
    let samePlace = false;
    try { samePlace = of.kind === 'place-chat' && ['org', 'project', 'topic', 'thread'].includes(pieces[1] ?? '') && decodeURIComponent(pieces[2] ?? '') === of.id; } catch { /* Malformed old locators do not select another resource. */ }
    if (samePlace || of.kind === 'chat' && previous.searchParams.get('chat') === of.id) {
      const message = previous.searchParams.get('message');
      if (message !== null && message !== '') url.searchParams.set('message', message);
    }
  }
  const anchorKey = of.kind === 'thread' ? 'item' : of.kind === 'document' ? 'block' : 'message';
  if (of.anchor !== undefined && of.anchor !== '') url.searchParams.set(anchorKey, of.anchor);
  if (of.kind === 'thread') {
    if (of.continuation === '') url.searchParams.delete('continuation');
    else if (of.continuation !== undefined) url.searchParams.set('continuation', of.continuation);
    if (of.prompt !== undefined && of.prompt !== '') url.searchParams.set('prompt', of.prompt);
  }
  if (of.kind === 'document' && of.version !== undefined) url.searchParams.set('version', of.version);
  return { ...tab, view: 'page', of: { href: url.pathname + url.search + url.hash } };
}
