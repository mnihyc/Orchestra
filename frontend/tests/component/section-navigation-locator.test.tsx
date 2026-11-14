/** Section names are real scoped links in one list or strip; pressing one follows its native destination. */
import { cleanup, screen, waitFor, within } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, useRouterState } from '@tanstack/react-router';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';

import { LISTED } from '@/features/admin/sections';
import { managementSections, placeSettingsSections, settingsNavigation } from '@/features/settings/navigation';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { i18n, setLocale } from '@/i18n';
import { SectionNav } from '@/ui/SectionNav';
import { show, skipOverflowFallback } from './support';

skipOverflowFallback();

const modes = [['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const;
const histories: ReturnType<typeof createMemoryHistory>[] = [];
const themeBefore = document.documentElement.dataset.theme;
const darkBefore = document.documentElement.classList.contains('dark');

afterEach(async () => {
  cleanup();
  for (const history of histories.splice(0)) history.destroy();
  if (themeBefore === undefined) delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = themeBefore;
  document.documentElement.classList.toggle('dark', darkBefore);
  await setLocale('en');
});

function ManagementLocator() {
  const selected = useRouterState({ select: (state) => state.location.pathname.split('/')[2] });
  const current = LISTED.find((entry) => entry.id === selected)?.id ?? 'overview';
  return <SectionNav label={i18n.t('admin.nav.label')} current={current} items={managementSections(i18n.t)} />;
}

function ScopedLocator() {
  const location = useRouterState({ select: (state) => state.location });
  const scope = settingsNavigation(location.pathname, location.search);
  if (scope?.kind !== 'project' && scope?.kind !== 'topic') throw new Error('The fixture requires an actual place-settings route');
  return <SectionNav label="Scoped sections" current={scope.current} items={placeSettingsSections(scope, i18n.t)} />;
}

function draw(href: string) {
  const reads = vi.fn();
  const root = createRootRoute({ component: Outlet });
  const admin = createRoute({ getParentRoute: () => root, path: '/admin/$section', component: ManagementLocator, loader: () => { reads(); return null; } });
  const topic = createRoute({ getParentRoute: () => root, path: '/topic/$topicId', component: ScopedLocator,
    validateSearch: (search: Record<string, unknown>) => search, loaderDeps: ({ search }) => search, loader: () => { reads(); return null; } });
  const project = createRoute({ getParentRoute: () => root, path: '/project/$projectId/settings', component: ScopedLocator,
    validateSearch: (search: Record<string, unknown>) => search, loaderDeps: ({ search }) => search, loader: () => { reads(); return null; } });
  const history = createMemoryHistory({ initialEntries: [href] }); histories.push(history);
  const router = createRouter({ routeTree: root.addChildren([admin, topic, project]), history, defaultPreload: false, defaultPendingMinMs: 0 });
  show(<RouterProvider router={router} />);
  return { router, reads };
}

function current(nav: HTMLElement, expected: string) {
  const links = within(nav).getAllByRole('link').filter((link) => link.getAttribute('aria-current') === 'page');
  expect(links).toHaveLength(1);
  expect(links[0]).toHaveAccessibleName(expected);
}

it.each(modes)('lists all 24 Management destinations as links in one list or strip, never a dropdown, and goes to a pressed one in %s/%s', async (locale, theme) => {
  await setLocale(locale); document.documentElement.dataset.theme = theme; document.documentElement.classList.toggle('dark', theme === 'dark');
  const user = userEvent.setup(); const { router, reads } = draw('/admin/overview');
  const label = i18n.t('admin.nav.label'); const nav = await screen.findByRole('navigation', { name: label });
  expect(within(nav).queryByRole('combobox')).not.toBeInTheDocument();
  expect(LISTED.map((entry) => entry.id)).toEqual([
    'overview', 'users', 'tokens', 'orgs', 'everything', 'sign-in', 'email', 'hosts', 'runtimes', 'templates', 'mcp',
    'capabilities', 'harnesses', 'agents', 'plugins', 'defaults', 'rates', 'settings', 'storage', 'semantic-search',
    'system', 'gateways', 'pki', 'audit',
  ]);
  expect(LISTED).toHaveLength(24);
  expect(within(nav).getAllByRole('link')).toHaveLength(24);
  for (const entry of LISTED) expect(within(nav).getByRole('link', { name: i18n.t(`admin.sections.${entry.id}.title`) })).toHaveAttribute('href', `/admin/${entry.id}`);
  const initial = i18n.t('admin.sections.overview.title'); const last = i18n.t('admin.sections.audit.title');
  current(nav, initial); expect(reads).toHaveBeenCalledTimes(1);
  // The list scrolls by itself where it is longer than the window; where narrow, the same names are a row that scrolls sideways.
  expect(nav.querySelector('ul')).toHaveClass('overflow-x-auto');
  await user.click(within(nav).getByRole('link', { name: last }));
  await waitFor(() => expect(router.state.location.pathname).toBe('/admin/audit'));
  await waitFor(() => expect(router.state.status).toBe('idle'));
  const settled = screen.getByRole('navigation', { name: label }); current(settled, last);
  expect(within(settled).queryByRole('combobox')).not.toBeInTheDocument(); expect(reads).toHaveBeenCalledTimes(2);
});

const scopes = [
  { href: '/topic/topic-place?tab=settings&section=defaults', destination: '/topic/topic-place?tab=settings&section=audit', search: { tab: 'settings', section: 'audit' } },
  { href: '/project/project-place/settings?section=defaults', destination: '/project/project-place/settings?section=audit', search: { section: 'audit' } },
] as const;

for (const scope of scopes) it.each(['en', 'zh-CN'] as const)(`keeps the exact scoped native destination from ${scope.href} in %s`, async (locale) => {
  await setLocale(locale); const user = userEvent.setup(); const { router, reads } = draw(scope.href);
  const nav = await screen.findByRole('navigation', { name: 'Scoped sections' }); const name = sectionLabel('audit', i18n.t);
  expect(within(nav).queryByRole('combobox')).not.toBeInTheDocument(); current(nav, sectionLabel('defaults', i18n.t));
  expect(within(nav).getByRole('link', { name })).toHaveAttribute('href', scope.destination);
  expect(router.state.location.href).toBe(scope.href); expect(reads).toHaveBeenCalledTimes(1);
  await user.click(within(nav).getByRole('link', { name })); await waitFor(() => expect(router.state.location.href).toBe(scope.destination));
  expect(router.state.location.search).toEqual(scope.search); expect(reads).toHaveBeenCalledTimes(2);
  const changed = await screen.findByRole('navigation', { name: 'Scoped sections' });
  current(changed, name); expect(within(changed).queryByRole('combobox')).not.toBeInTheDocument();
});
