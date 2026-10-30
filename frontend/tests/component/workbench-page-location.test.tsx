/** A retained page's locator stays complete and stable when attention changes the outer URL. */
import { renderHook } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import type * as Router from '@tanstack/react-router';
import { PageContext, useViewLocation } from '@/features/workbench/pageContext';
import type { PageTarget } from '@/features/workbench/pages';

const fixture = vi.hoisted(() => ({ location: { pathname: '/search', search: { q: 'another pane' }, searchStr: '?q=another%20pane',
  hash: 'foreign', href: '/search?q=another%20pane#foreign', publicHref: '/search?q=another%20pane#foreign', external: false, state: { __TSR_index: 42, privateHistoryFact: 'outer page only' } } }));
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(), useLocation: () => fixture.location }));

it('returns its own href, raw search, hash and parsed search without inheriting another page’s history state', () => {
  const target: PageTarget = { route: '/thread/$threadId/', pathname: '/thread/own-thread', params: { threadId: 'own-thread' }, search: { setting: 'effort' }, hash: 'own-anchor' };
  const wrapper = ({ children }: { children: ReactNode }) => <PageContext.Provider value={{ id: 'own-tab', target, navigate: () => Promise.resolve() }}>{children}</PageContext.Provider>;
  const hook = renderHook(() => useViewLocation(), { wrapper });
  expect(hook.result.current).toEqual({ pathname: '/thread/own-thread', search: { setting: 'effort' }, searchStr: '?setting=effort', hash: 'own-anchor',
    href: '/thread/own-thread?setting=effort#own-anchor', publicHref: '/thread/own-thread?setting=effort#own-anchor', external: false, state: { __TSR_index: 0 } });
  const own = hook.result.current;
  fixture.location = { ...fixture.location, pathname: '/help', href: '/help', searchStr: '', hash: '' };
  hook.rerender(); expect(hook.result.current).toBe(own);
});

it('keeps the outer router location for actual shell consumers', () => {
  const hook = renderHook(() => useViewLocation());
  expect(hook.result.current).toBe(fixture.location);
});
