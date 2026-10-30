/** Native router activity must agree with the controlled section; every header uses the canonical Actions sign. */
import { DndContext } from '@dnd-kit/core';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { act, cleanup, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { PanelsTopLeft } from 'lucide-react';
import type { ReactNode } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import type * as Mode from '@/api/adminMode';
import { adminModeKey } from '@/api/adminMode';
import { AreaSections } from '@/features/navigation/AreaSections';
import { NavigationActions } from '@/features/navigation/Sidebar';
import { SettingsSidebar } from '@/features/settings/Sidebar';
import { useSettingsNavigation } from '@/features/settings/navigation';
import { readSearch } from '@/features/usage/search';
import { SideBarView } from '@/features/workbench/Sections';
import { makeContainer, makeTab } from '@/features/workbench/layout';
import { useWorkbench } from '@/features/workbench/store';
import { registerView } from '@/features/workbench/views';
import { i18n, setLocale } from '@/i18n';
import { clearScreenHeading } from '@/shell/screen';
import { show } from './support';

const reads = vi.hoisted(() => vi.fn(() => Promise.resolve({ as_admin: false, available: true })));
vi.mock('@/api/adminMode', async (original) => ({ ...await original<typeof Mode>(), adminModeQuery: { queryKey: ['admin-mode'], queryFn: reads } }));
const before = useWorkbench.getState();
const caches: QueryClient[] = [];
const undo: (() => void)[] = [];
afterEach(async () => { cleanup(); for (const cache of caches.splice(0)) cache.clear(); for (const remove of undo.splice(0)) remove(); useWorkbench.setState(before); clearScreenHeading(); reads.mockClear(); document.documentElement.removeAttribute('data-theme'); await setLocale('en'); });

function AdminSections() {
  const scope = useSettingsNavigation();
  return scope?.kind === 'system' ? <SettingsSidebar scope={scope} /> : null;
}
function draw(path: string, ui?: ReactNode) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }); caches.push(cache);
  cache.setQueryData(adminModeKey, { as_admin: false, available: true });
  const root = createRootRoute({ component: () => ui ?? (path.startsWith('/usage') ? <AreaSections area="usage" /> : <AdminSections />) });
  const routes = [
    createRoute({ getParentRoute: () => root, path: '/usage', validateSearch: readSearch }),
    createRoute({ getParentRoute: () => root, path: '/admin' }),
    createRoute({ getParentRoute: () => root, path: '/admin/$section' }),
    createRoute({ getParentRoute: () => root, path: '/settings/profile' }),
    createRoute({ getParentRoute: () => root, path: '/help' }),
    createRoute({ getParentRoute: () => root, path: '/org/$slug/settings/$section' }),
  ];
  const router = createRouter({ routeTree: root.addChildren(routes), history: createMemoryHistory({ initialEntries: [path] }) });
  const view = show(<QueryClientProvider client={cache}><RouterProvider router={router} /></QueryClientProvider>);
  return { cache, router, view };
}
function current(nav: HTMLElement, expected: string) {
  const active = nav.querySelectorAll('[data-section-id][aria-current="page"]');
  expect(active).toHaveLength(1); expect(active[0]).toHaveAttribute('data-section-id', expected);
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`marks one native Usage destination, preserving scope/dates/filters through pointer and keyboard at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    const { router } = draw('/usage?view=performance&scope=node:00000000-0000-4000-8000-000000000001&range=last_week&model=Swift');
    const nav = await screen.findByRole('navigation', { name: i18n.t('usagePerf.views') });
    current(nav, 'performance');
    await user.click(within(nav).getByRole('link', { name: i18n.t('usagePerf.viewNames.requests') }));
    await waitFor(() => current(nav, 'requests'));
    expect(router.state.location.search).toEqual(expect.objectContaining({ view: 'requests', scope: 'node:00000000-0000-4000-8000-000000000001', range: 'last_week', model: 'Swift' }));
    const usage = within(nav).getByRole('link', { name: i18n.t('usagePerf.viewNames.usage') });
    usage.focus(); await user.keyboard('{Enter}');
    await waitFor(() => current(nav, 'usage')); expect(router.state.location.search.view).toBe('usage');
    expect(reads).not.toHaveBeenCalled();
  });

  it(`names the actual access gate while Off and the actual section while On at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme;
    const { cache } = draw('/admin');
    const nav = await screen.findByRole('navigation', { name: i18n.t('admin.nav.label') });
    current(nav, 'access'); expect(within(nav).getAllByRole('link')).toHaveLength(1);
    expect(within(nav).getByRole('link', { name: i18n.t('workbench.areaNavigation.system.entry.name') })).toHaveAttribute('href', '/admin');
    expect(within(nav).queryByRole('link', { name: i18n.t('admin.sections.overview.title') })).not.toBeInTheDocument();
    act(() => { cache.setQueryData(adminModeKey, { as_admin: true, available: true }); });
    await screen.findByRole('link', { name: i18n.t('admin.sections.overview.title') }); current(nav, 'overview');
    const user = userEvent.setup(); const users = within(nav).getByRole('link', { name: i18n.t('admin.sections.users.title') });
    users.focus(); await user.keyboard('{Enter}'); await waitFor(() => current(nav, 'users'));
    act(() => { cache.setQueryData(adminModeKey, { as_admin: false, available: true }); });
    await screen.findByRole('link', { name: i18n.t('workbench.areaNavigation.system.entry.name') });
    current(nav, 'access'); expect(within(nav).getAllByRole('link')).toHaveLength(1);
    expect(reads).not.toHaveBeenCalled();
  });

  it(`uses one real Actions menu sign for every left-area title at ${locale}/${theme}`, async () => {
    await setLocale(locale); document.documentElement.dataset.theme = theme; clearScreenHeading();
    undo.push(registerView({ name: 'navigation', title: 'views.navigation.title', about: 'views.navigation.about', icon: PanelsTopLeft, screens: 'all', area: 'left', body: () => null, actions: NavigationActions, menu: NavigationActions }));
    const left = { ...before.layout.left, active: 'navigation', containers: [makeContainer('navigation', [makeTab('navigation')])] };
    useWorkbench.setState({ ...before, layout: { ...before.layout, left } });
    for (const path of ['/settings/profile', '/help', '/usage', '/admin', '/org/acme/settings/general']) {
      const { view } = draw(path, <DndContext><SideBarView bar="left" layout={left} renderView={() => null} addView={<span />} /></DndContext>);
      await waitFor(() => expect(view.container.querySelector('[data-container-title]')).toBeInTheDocument());
      const title = view.container.querySelector('[data-container-title]');
      if (!(title instanceof HTMLElement)) throw new Error('The left bar has no title');
      const controls = title.querySelectorAll<HTMLButtonElement>('button[aria-haspopup="menu"]');
      expect(controls).toHaveLength(1); const button = controls[0];
      if (button === undefined) throw new Error('The title has no menu');
      expect(button).toHaveAttribute('data-action', 'actions');
      expect(button.querySelector('svg')).toHaveClass('lucide-ellipsis');
      expect(button.querySelectorAll('svg circle')).toHaveLength(3);
      await userEvent.click(button);
      expect(await screen.findByRole('menuitem', { name: i18n.t('workbench.section.hide') })).toBeVisible();
      await userEvent.keyboard('{Escape}'); view.unmount();
    }
  });
}
