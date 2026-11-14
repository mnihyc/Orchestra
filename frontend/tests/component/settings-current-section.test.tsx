/** Real router matching must not add General's aria-current to a different selected section. */
import { act, cleanup, screen, within } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, useRouterState } from '@tanstack/react-router';
import { afterEach, expect, it } from 'vitest';
import { settingsDestinations } from '@/features/finder/settingsCommands';
import { SettingsFrame } from '@/features/settings/Frame';
import { placeSettingsSections } from '@/features/settings/navigation';
import { isProjectSection, isTopicSection } from '@/features/settings/ScopeSettings';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { canonicalPageAddress } from '@/features/workbench/PageView';
import { i18n, setLocale } from '@/i18n';
import { show } from './support';

afterEach(async () => { cleanup(); await setLocale('en'); });

function TopicSettings() {
  const search = useRouterState({ select: (state) => state.location.search });
  const current = isTopicSection(search.section) ? search.section : 'general';
  return <SettingsFrame title={sectionLabel(current, i18n.t)} label="Topic sections" current={current}
    items={placeSettingsSections({ kind: 'topic', id: 'login', current }, i18n.t)}><p>Topic settings content</p></SettingsFrame>;
}
function ProjectSettings() {
  const search = useRouterState({ select: (state) => state.location.search });
  const current = isProjectSection(search.section) ? search.section : 'general';
  return <SettingsFrame title={sectionLabel(current, i18n.t)} label="Project sections" current={current}
    items={placeSettingsSections({ kind: 'project', id: 'web', current }, i18n.t)}><p>Project settings content</p></SettingsFrame>;
}

it.each(['en', 'zh-CN'] as const)('marks exactly one current section for topic/project links, including General’s normalized search in %s', async (locale) => {
  await setLocale(locale);
  const root = createRootRoute({ component: Outlet });
  const topic = createRoute({ getParentRoute: () => root, path: '/topic/$topicId', component: TopicSettings,
    validateSearch: (search: Record<string, unknown>) => ({ tab: 'settings', ...(isTopicSection(search.section) && search.section !== 'general' ? { section: search.section } : {}) }),
  });
  const project = createRoute({ getParentRoute: () => root, path: '/project/$projectId/settings', component: ProjectSettings,
    validateSearch: (search: Record<string, unknown>) => isProjectSection(search.section) && search.section !== 'general' ? { section: search.section } : {},
  });
  const router = createRouter({ routeTree: root.addChildren([topic, project]), history: createMemoryHistory({ initialEntries: ['/topic/login?tab=settings&section=defaults'] }), defaultPendingMinMs: 0 });
  show(<RouterProvider router={router} />);
  const assertCurrent = async (label: string, section: string) => {
    const nav = await screen.findByRole('navigation', { name: label });
    const selected = nav.querySelectorAll('[aria-current="page"]');
    expect(selected).toHaveLength(1);
    expect(selected[0]).toHaveTextContent(sectionLabel(section, i18n.t));
    expect(within(nav).queryByRole('combobox')).not.toBeInTheDocument();
  };
  await assertCurrent('Topic sections', 'defaults');
  await act(async () => { await router.navigate({ to: '/topic/$topicId', params: { topicId: 'login' }, search: { tab: 'settings', section: 'general' } }); });
  await assertCurrent('Topic sections', 'general');
  await act(async () => { await router.navigate({ to: '/topic/$topicId', params: { topicId: 'login' }, search: { tab: 'settings', section: 'audit' } }); });
  await assertCurrent('Topic sections', 'audit');
  await act(async () => { await router.navigate({ to: '/project/$projectId/settings', params: { projectId: 'web' }, search: { section: 'defaults' } }); });
  await assertCurrent('Project sections', 'defaults');
  await act(async () => { await router.navigate({ to: '/project/$projectId/settings', params: { projectId: 'web' }, search: { section: 'general' } }); });
  await assertCurrent('Project sections', 'general');
});

it.each(['en', 'zh-CN'] as const)('has the first section of a place\u2019s Settings written as the address without a section by every writer, and an address that names it opened as the same screen in %s', async (locale) => {
  await setLocale(locale);
  // The writers: the links of the frame and its sidebar, and the finder\u2019s commands for each section.
  const root = createRootRoute({ component: Outlet });
  const router = createRouter({ routeTree: root.addChildren([
    createRoute({ getParentRoute: () => root, path: '/topic/$topicId', component: () => null }),
    createRoute({ getParentRoute: () => root, path: '/project/$projectId/settings', component: () => null }),
  ]), history: createMemoryHistory({ initialEntries: ['/'] }), defaultPendingMinMs: 0 });
  const hrefs = (kind: 'topic' | 'project') => placeSettingsSections({ kind, id: 'x', current: 'general' }, i18n.t)
    .map((item) => [item.id, (router.buildLocation(item.link.props as never) as { href: string }).href]);
  expect(Object.fromEntries(hrefs('topic'))).toMatchObject({ general: '/topic/x?tab=settings', defaults: '/topic/x?tab=settings&section=defaults' });
  expect(Object.fromEntries(hrefs('project'))).toMatchObject({ general: '/project/x/settings', defaults: '/project/x/settings?section=defaults' });
  const commands = (kind: 'topic' | 'project') => Object.fromEntries(settingsDestinations({ id: 'x', name: 'X', kind }, i18n.t).map((command) => [command.id, command.path]));
  expect(commands('topic')).toMatchObject({ general: '/topic/x?tab=settings', defaults: '/topic/x?tab=settings&section=defaults' });
  expect(commands('project')).toMatchObject({ general: '/project/x/settings', defaults: '/project/x/settings?section=defaults' });
  // An address that names it, typed or kept from before, is the same screen and is written without it.
  expect(await canonicalPageAddress('/topic/x?tab=settings&section=general')).toBe('/topic/x?tab=settings');
  expect(await canonicalPageAddress('/project/x/settings?section=general')).toBe('/project/x/settings');
  expect(await canonicalPageAddress('/topic/x?tab=settings&section=defaults')).toBe('/topic/x?tab=settings&section=defaults');
  expect(await canonicalPageAddress('/project/x/settings?section=defaults')).toBe('/project/x/settings?section=defaults');
  expect(await canonicalPageAddress('/topic/x?tab=settings')).toBe('/topic/x?tab=settings');
});
