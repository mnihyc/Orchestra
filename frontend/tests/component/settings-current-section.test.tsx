/** Real router matching must not add General's aria-current to a different selected section. */
import { act, cleanup, screen, within } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, useRouterState } from '@tanstack/react-router';
import { afterEach, expect, it } from 'vitest';
import { SettingsFrame } from '@/features/settings/Frame';
import { placeSettingsSections } from '@/features/settings/navigation';
import { isProjectSection, isTopicSection } from '@/features/settings/ScopeSettings';
import { sectionLabel } from '@/features/settings/sectionLabels';
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
  const router = createRouter({ routeTree: root.addChildren([topic, project]), history: createMemoryHistory({ initialEntries: ['/topic/login?tab=settings&section=policies'] }), defaultPendingMinMs: 0 });
  show(<RouterProvider router={router} />);
  const assertCurrent = async (label: string, section: string) => {
    const nav = await screen.findByRole('navigation', { name: label });
    const selected = nav.querySelectorAll('[aria-current="page"]');
    expect(selected).toHaveLength(1);
    expect(selected[0]).toHaveTextContent(sectionLabel(section, i18n.t));
    expect(within(nav).getByRole('combobox', { name: label })).toHaveValue(sectionLabel(section, i18n.t));
  };
  await assertCurrent('Topic sections', 'policies');
  await act(async () => { await router.navigate({ to: '/topic/$topicId', params: { topicId: 'login' }, search: { tab: 'settings', section: 'general' } }); });
  await assertCurrent('Topic sections', 'general');
  await act(async () => { await router.navigate({ to: '/topic/$topicId', params: { topicId: 'login' }, search: { tab: 'settings', section: 'audit' } }); });
  await assertCurrent('Topic sections', 'audit');
  await act(async () => { await router.navigate({ to: '/project/$projectId/settings', params: { projectId: 'web' }, search: { section: 'policies' } }); });
  await assertCurrent('Project sections', 'policies');
  await act(async () => { await router.navigate({ to: '/project/$projectId/settings', params: { projectId: 'web' }, search: { section: 'general' } }); });
  await assertCurrent('Project sections', 'general');
});
