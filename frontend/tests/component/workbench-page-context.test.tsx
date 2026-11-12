/** Index-route spelling changes neither the addressed page nor its local typed adapters. */
import { renderHook } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter } from '@tanstack/react-router';
import { expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import { PageContext, usePageNavigate, usePageParams, usePageSearch, type PageContextValue } from '@/features/workbench/pageContext';
import type { PageTarget } from '@/features/workbench/pages';

function routes() {
  const root = createRootRoute();
  const providers = createRoute({ getParentRoute: () => root, path: '/resources/providers',
    validateSearch: (value: Record<string, unknown>) => ({ q: typeof value.q === 'string' ? value.q : '' }) });
  const providersIndex = createRoute({ getParentRoute: () => providers, path: '/' });
  const org = createRoute({ getParentRoute: () => root, path: '/org/$slug' });
  const orgIndex = createRoute({ getParentRoute: () => org, path: '/' });
  const hosts = createRoute({ getParentRoute: () => root, path: '/resources/hosts' });
  const home = createRoute({ getParentRoute: () => root, path: '/' });
  createRouter({ routeTree: root.addChildren([providers.addChildren([providersIndex]), org.addChildren([orgIndex]), hosts, home]),
    history: createMemoryHistory({ initialEntries: ['/resources/providers'] }) });
  return { providers, providersIndex, org, orgIndex, hosts, home };
}
function hosted(target: PageTarget) {
  const navigate: PageContextValue['navigate'] = vi.fn(() => Promise.resolve());
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext.Provider value={{ id: 'addressed-page', target, navigate }}>{children}</PageContext.Provider>;
  return { wrapper, navigate };
}

it('uses the same params, search and navigation for the router’s index and trailing-slash-free route ids', () => {
  const { providers, providersIndex } = routes();
  expect(providers.id).toBe('/resources/providers');
  expect(providersIndex.id).toBe('/resources/providers/');
  const target: PageTarget = { route: '/resources/providers/', pathname: '/resources/providers', params: {}, search: { q: 'retained' }, hash: '' };
  const { wrapper, navigate } = hosted(target);
  const hook = renderHook(() => ({ params: usePageParams(providers), search: usePageSearch(providers), navigate: usePageNavigate(providers),
    indexSearch: usePageSearch(providersIndex) }), { wrapper });
  expect(hook.result.current.params).toBe(target.params);
  expect(hook.result.current.search).toBe(target.search);
  expect(hook.result.current.indexSearch).toBe(target.search);
  expect(hook.result.current.navigate).toBe(navigate);
});

it('preserves a parameterized org index’s own slug and the root index', () => {
  const { org, orgIndex, home } = routes();
  const orgTarget: PageTarget = { route: '/org/$slug/', pathname: '/org/acme', params: { slug: 'acme' }, search: { tab: 'activity' }, hash: '' };
  const orgHost = hosted(orgTarget);
  const hook = renderHook(() => ({ parent: usePageParams(org), index: usePageParams(orgIndex) }), { wrapper: orgHost.wrapper });
  expect(hook.result.current.parent).toEqual({ slug: 'acme' });
  expect(hook.result.current.index).toBe(orgTarget.params);
  const homeHost = hosted({ route: '/', pathname: '/', params: {}, search: {}, hash: '' });
  const rootHook = renderHook(() => usePageParams(home), { wrapper: homeHost.wrapper });
  expect(rootHook.result.current).toEqual({});
});

it('still refuses a different resource family and an unhosted page', () => {
  const { providers, hosts } = routes();
  const { wrapper } = hosted({ route: '/resources/providers/', pathname: '/resources/providers', params: {}, search: {}, hash: '' });
  expect(() => renderHook(() => usePageParams(hosts), { wrapper })).toThrow('This page must be hosted by its addressed workbench view.');
  expect(() => renderHook(() => usePageParams(providers))).toThrow('This page must be hosted by its addressed workbench view.');
});
