/** Typed route adapters read their own content target. There is still one application router. */
import { defaultStringifySearch, useLocation, useNavigate, type AnyRoute, type UseNavigateResult, type ParsedLocation } from '@tanstack/react-router';
import { createContext, useContext, useMemo } from 'react';
import type { PageTarget } from './pages';

export interface PageNavigation {
  to?: string;
  href?: string;
  params?: Record<string, string> | ((before: Record<string, string>) => Record<string, string>);
  search?: Record<string, unknown> | true | ((before: Record<string, unknown>) => Record<string, unknown>);
  hash?: string;
  replace?: boolean;
}
export interface PageContextValue {
  id: string;
  target: PageTarget;
  navigate: (options: PageNavigation) => Promise<void>;
}
export const PageContext = createContext<PageContextValue | null>(null);
export function usePageContext(): PageContextValue | null { return useContext(PageContext); }
function useTarget(route: { id: string }): PageContextValue {
  const context = usePageContext();
  if (context?.target.route !== route.id) throw new Error('This page must be hosted by its addressed workbench view.');
  return context;
}
export function usePageParams<R extends AnyRoute>(route: R): R['types']['allParams'] {
  return useTarget(route).target.params;
}
export function usePageSearch<R extends AnyRoute>(route: R): R['types']['fullSearchSchema'] {
  return useTarget(route).target.search;
}
export function usePageNavigate<R extends AnyRoute>(route: R): UseNavigateResult<R['types']['fullPath']> {
  return useTarget(route).navigate as UseNavigateResult<R['types']['fullPath']>;
}

/** Feature controls inside a page read that page's locator; shell consumers retain the outer route. */
export function useViewLocation<Value = ParsedLocation>(options?: { select?: (location: ParsedLocation) => Value }): Value {
  const outer = useLocation();
  const target = usePageContext()?.target;
  const addressed = useMemo((): ParsedLocation | null => {
    if (target === undefined) return null;
    const searchStr = defaultStringifySearch(target.search);
    const href = `${target.pathname}${searchStr}${target.hash === '' ? '' : `#${target.hash}`}`;
    // A pane has one local locator, not an independent browser-history stack or another pane's history state.
    return { pathname: target.pathname, search: target.search, hash: target.hash, searchStr, href, publicHref: href, external: false, state: { __TSR_index: 0 } };
  }, [target]);
  const own = addressed ?? outer;
  return options?.select === undefined ? own as Value : options.select(own);
}
export function useViewNavigate(): ReturnType<typeof useNavigate> {
  const outer = useNavigate();
  return (usePageContext()?.navigate ?? outer) as ReturnType<typeof useNavigate>;
}
