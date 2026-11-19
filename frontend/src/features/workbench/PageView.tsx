/** Full existing feature screens, each with its own typed target and stable mounted tab identity. */
import { placeChatAddress } from '@/features/chat/targets';
import { ViewInstanceContext } from './instance';
import { useFrameReady } from '@/shell/FrameReadiness';
import { canonicalOrgAddress, isOrgIdAddress } from '@/features/navigation/orgAddress';
import { isSection } from '@/features/admin/sections';
import { NotFound } from '@/routes/not-found';
import { markPageNavigation, usePageFailures, setPageFailure } from './pageNavigation';
import { defaultStringifySearch, isNotFound, notFound, useRouter } from '@tanstack/react-router';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ErrorState } from '@/ui/ErrorState';
import { SkeletonRows } from '@/ui/Skeleton';
import { ContentBoundary } from '@/ui/ContentBoundary';
import { ThreadReadProvider } from '@/realtime/threadReadContext';
import { contentThreadRead } from '@/realtime/threadScope';
import { setTabTarget, findTab, type Tab } from './layout';
import { PageContext, type PageNavigation } from './pageContext';
import { pageDefinition, pageParam, type PageDefinition, type PageTarget } from './pages';
import { contentAddress, contentLocation, contentTarget, pageHref, parsePageAddress, type ContentLocation } from './contentAddress';
import { openContent } from './contentNavigation';
import { WorkspaceContext } from './WorkspaceContext';
import { PageFileIntent } from './PageFileIntent';
export { EMPTY_WORKBENCH, parsePageAddress, pageResource } from './contentAddress';
import { useWorkbench } from './store';
import type { ViewProps } from './views';
import { TargetContext } from '@/shell/RouteContext';
import { isListRoute, storedListState, usePageListSearch } from '@/lib/listState';
import { readViewChoice } from '@/lib/viewChoices';
import { client } from '@/api/client';

/** Compatibility name used by existing full-page callers; all full content now has an address. */
export const pageAddress = contentAddress;
/** Canonical aliases resolve before a body mounts, including tabs restored or opened beside. */
async function resolveTarget(target: PageTarget): Promise<PageTarget> {
  if (target.route === '/place-chat/$nodeId/') {
    const href = await placeChatAddress(pageParam(target, 'nodeId'), typeof target.search.message === 'string' ? target.search.message : undefined);
    const resolved = parsePageAddress(href); if (resolved === null) throw notFound(); return resolved;
  }
  if (target.route === '/') {
    const { landingAddress } = await import('@/routes/index');
    const href = await landingAddress();
    if (href !== null && href !== '/') { const next = parsePageAddress(href); if (next === null) throw notFound(); return resolveTarget(next); }
  }
  if (target.route === '/admin/$section/' && !isSection(pageParam(target, 'section'))) throw notFound();
  if (target.route === '/org/$slug/' || target.route === '/org/$slug/settings/$section/') {
    const slug = pageParam(target, 'slug');
    const address = await canonicalOrgAddress(slug);
    if (address !== undefined) return { ...target, pathname: target.pathname.replace(`/org/${encodeURIComponent(slug)}`, `/org/${encodeURIComponent(address)}`), params: { ...target.params, slug: address } };
  }
  return target;
}
const hrefOf = pageHref;
export async function canonicalPageAddress(href: string): Promise<string> {
  const target = parsePageAddress(href);
  return target === null ? href : hrefOf(await resolveTarget(target));
}


/** The compatibility entry now accepts the same typed file/difference addresses. */
export const openPage = openContent;

export function PageView({ tab }: ViewProps) {
  const router = useRouter();
  const routeFailure = usePageFailures((state) => state.byTab.get(tab.id));
  const href = pageAddress(tab);
  const target = useMemo(() => href === null ? null : parsePageAddress(href), [href]);
  const [loaded, setLoaded] = useState<{ route: string; pathname: string; definition: PageDefinition } | null>(null);
  const [failure, setFailure] = useState<{ pathname: string; value: unknown } | null>(null);
  const error = failure?.pathname === target?.pathname ? failure?.value ?? null : null;
  const [attempt, retry] = useState(0);
  useEffect(() => {
    if (target === null || routeFailure !== undefined) return;
    const request = new AbortController();
    const current = () => !request.signal.aborted;
    void resolveTarget(target).then(async (resolved) => {
      if (!current()) return;
      if (resolved.pathname !== target.pathname || hrefOf(resolved) !== href) {
        const canonical = hrefOf(resolved);
        if (useWorkbench.getState().activeContent === tab.id) markPageNavigation(tab.id, canonical);
        useWorkbench.getState().change((layout) => setTabTarget(layout, tab.id, { href: canonical }));
        return;
      }
      const definition = await pageDefinition(target.route);
      if (current()) setLoaded({ route: target.route, pathname: target.pathname, definition });
    }).catch((failure: unknown) => { if (current()) setFailure({ pathname: target.pathname, value: failure }); });
    return () => request.abort();
  }, [target, routeFailure, attempt, tab.id, href]);
  if (routeFailure !== undefined) return isNotFound(routeFailure) ? <NotFound /> : <ErrorState error={routeFailure} onRetry={() => {
    setPageFailure(tab.id, undefined);
    if (target !== null && useWorkbench.getState().activeContent === tab.id) void router.invalidate({ filter: (match) => match.routeId === target.route });
    retry((value) => value + 1);
  }} />;
  if (isNotFound(error)) return <NotFound />;
  if (error !== null) return <ErrorState error={error} onRetry={() => { setFailure(null); retry((value) => value + 1); }} />;
  if (target === null) return <NotFound />;
  if (loaded?.route !== target.route || loaded.pathname !== target.pathname) return <SkeletonRows rows={5} />;
  return <ContentBoundary resetKey={`${target.route}:${target.pathname}:${attempt}`} className="h-full"><LoadedPage tab={tab} target={target} definition={loaded.definition} /></ContentBoundary>;
}

function LoadedPage({ tab, target, definition }: { tab: Tab; target: PageTarget; definition: PageDefinition }) {
  const router = useRouter();
  const search = usePageListSearch(target.route, target.pathname, target.search, definition.validate);
  const latestSearch = useRef(search);
  useLayoutEffect(()=>{latestSearch.current=search;},[search]);
  const validated = useMemo(() => ({ ...target, search }), [target, search]);
  const navigate = useCallback(async (options: PageNavigation) => {
    // Every callback belongs to this still-open instance, never whichever route later has focus.
    const current = findTab(useWorkbench.getState().layout, tab.id);
    if (current === null) return;
    const address=pageAddress(current.tab);
    const owned=address===null?null:parsePageAddress(address);
    if(owned?.route!==target.route||owned.pathname!==target.pathname)return;
    let href = options.href;
    if (href === undefined) {
      const params = typeof options.params === 'function' ? options.params(target.params) : options.params ?? target.params;
      const to = (options.to ?? target.pathname).replace(/\$([A-Za-z][A-Za-z0-9_]*)/g, (_, name: string) => encodeURIComponent(params[name] ?? ''));
      let nextSearch = options.search === true || options.search === undefined ? latestSearch.current
        : typeof options.search === 'function' ? options.search(latestSearch.current) : options.search;
      if(target.route==='/topic/$topicId/'&&options.to===undefined&&nextSearch.tab!==latestSearch.current.tab) {
        const scope=nextSearch.tab==='settings'?`${target.pathname}.settings`:target.pathname;
        const kept=storedListState(readViewChoice(client.session.getState().userId,`page.${scope}`,'state','{}'));
        nextSearch={...kept,...nextSearch};
      }
      if (isListRoute(target.route) && (options.to === undefined || options.to === target.pathname)) {
        nextSearch = { ...nextSearch, _list: true, ...(nextSearch.list === undefined && latestSearch.current.list !== undefined ? { list: latestSearch.current.list } : {}) };
      }
      latestSearch.current = nextSearch;
      const hash = options.hash ?? (options.to === undefined ? target.hash : '');
      href = `${to}${defaultStringifySearch(nextSearch)}${hash === '' ? '' : `#${hash}`}`;
    }
    const next = parsePageAddress(href);
    if (next === null) { await router.navigate({ href }); return; }
    if (next.pathname === target.pathname || options.replace === true) {
      if (useWorkbench.getState().activeContent === tab.id) markPageNavigation(tab.id, href);
      useWorkbench.getState().change((layout) => setTabTarget(layout, tab.id, { href }));
      if (useWorkbench.getState().activeContent === tab.id) await router.navigate({ href, replace: true });
    } else {
      openPage(href);
      await router.navigate({ href });
    }
  }, [router, tab, target]);
  useEffect(() => {
    if (!isListRoute(target.route) || target.search._list !== undefined) return;
    const addressed = { ...definition.validate(target.search), ...(target.search.list === undefined ? {} : { list: target.search.list }), _list: undefined };
    const restored = { ...search, _list: undefined };
    if (JSON.stringify(addressed) !== JSON.stringify(restored)) void navigate({ search, replace: true });
  }, [navigate, search, target.route, target.search, definition]);
  const context = useMemo(() => ({ id: tab.id, target: validated, navigate }), [tab.id, validated, navigate]);
  const threadId = target.route === '/thread/$threadId/' ? pageParam(target, 'threadId') : null;
  const read = useMemo(() => threadId === null ? null : contentThreadRead(threadId, tab.id), [threadId, tab.id]);
  const Body = definition.Body;
  const body = <PageContext.Provider value={context}><TargetContext target={validated} /><PageFileIntent tab={tab} target={target} /><Body /></PageContext.Provider>;
  return read === null ? body : <ThreadReadProvider read={read}>{body}</ThreadReadProvider>;
}

/** An active content target can supply sidebar metadata without mounting its hidden conversation. */
export function FocusedContentMetadata() {
  const active = useWorkbench((state) => state.activeContent);
  const tab = useWorkbench((state) => active === null ? null : findTab(state.layout, active)?.tab ?? null);
  const ready = useFrameReady();
  const href = tab === null ? null : pageAddress(tab);
  const target = useMemo(() => href === null ? null : parsePageAddress(href), [href]);
  const root = useRef<HTMLDivElement>(null);
  const resource = useMemo(() => tab === null ? null : contentTarget(tab), [tab]);
  const location: ContentLocation | null = tab === null ? null : contentLocation(tab);
  const owner = tab?.id ?? '';
  const instance = useMemo(() => ({ id: owner, root, target: resource, headingOwner: owner, content: true, focusedContent: true }), [owner, resource]);
  const read = useMemo(() => resource?.kind === 'thread' ? contentThreadRead(resource.id, owner) : null, [resource, owner]);
  if (!ready || tab === null || target === null && resource?.workspace === undefined || target !== null && (target.route === '/org/$slug/' || target.route === '/org/$slug/settings/$section/') && isOrgIdAddress(target.params.slug ?? '')) return null;
  const body = <ViewInstanceContext.Provider value={instance}>{resource?.workspace !== undefined ? <WorkspaceContext source={resource.workspace} location={location} /> : target === null ? null : <TargetContext target={target} />}</ViewInstanceContext.Provider>;
  return read === null ? body : <ThreadReadProvider read={read}>{body}</ThreadReadProvider>;
}
