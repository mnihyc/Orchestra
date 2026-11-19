/** Page sections remain inside their page while the global navigation keeps the chosen org. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type * as Router from '@tanstack/react-router';
import type * as LiveModule from '@/realtime/live';
import { cleanup, fireEvent, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { AnchorHTMLAttributes, ReactNode } from 'react';
import { useState } from 'react';
import userEvent from '@testing-library/user-event';
import { Sidebar } from '@/features/navigation/Sidebar';
import { navigationRoot } from '@/features/navigation/currentOrg';
import { AreaSections } from '@/features/navigation/AreaSections';
import { helpSearch } from '@/features/help/navigation';
import { meKey } from '@/api/auth';
import { adminModeKey } from '@/api/adminMode';
import { orgsKey, orgBySlugQuery, projectQuery, topicQuery } from '@/api/nodes';
import type { Project, Topic } from '@/generated/api-types';
import { SettingsFrame } from '@/features/settings/Frame';
import { placeSettingsSections, settingsNavigation, type PlaceSettingsNavigation } from '@/features/settings/navigation';
import { ViewOpeningContext } from '@/features/workbench/viewOpening';
import { i18n, setLocale } from '@/i18n';
import { useAppearance } from '@/lib/appearance';
import { useSelectedOrg } from '@/shell/org';
import { me, org, userRef } from '../doubles/fixtures';
import { show } from './support';

const world = vi.hoisted(() => {
  const search: Record<string, unknown> = {};
  return { path: '/settings/profile', search, tree: vi.fn(), chats: vi.fn(), followed: vi.fn() };
});
vi.mock('@tanstack/react-router', async (load) => ({ ...await load<typeof Router>(),
  useRouterState: ({ select }: { select: (state: { location: { pathname: string; search: Record<string, unknown> } }) => unknown }) => select({ location: { pathname: world.path, search: world.search } }),
  Link: ({ to, search: selected, params, children, activeOptions: _activeOptions, onClick, ...props }: AnchorHTMLAttributes<HTMLAnchorElement> & { to: string; search?: Record<string, string | number | boolean | undefined>; params?: Record<string, string>; children?: ReactNode; activeOptions?: unknown }) => {
    const query = new URLSearchParams(Object.entries(selected ?? {}).flatMap(([key, value]): [string, string][] => value === undefined ? [] : [[key, String(value)]])).toString();
    const href = `${to.replace(/\$(\w+)/g, (_match: string, key: string) => params?.[key] ?? '')}${query === '' ? '' : `?${query}`}`;
    return <a {...props} href={href} onClick={(event) => { onClick?.(event); event.preventDefault(); world.followed(href); }}>{children}</a>;
  },
}));
vi.mock('@/realtime/live', async (load) => {
  const actual = await load<typeof LiveModule>();
  return { ...actual, useOrgTreeFeed: world.tree };
});
vi.mock('@/features/navigation/counts', async (load) => ({ ...await load<object>(), useUnreadChats: () => { world.chats(); return 0; } }));

const appearance = useAppearance.getState();
const person = { ...me(1, 'ada', 'Ada'), is_system_admin: true };
const remembered = org('team', 'acme', 'Acme', userRef(person));
const department = org('department', 'research', 'Research', userRef(person), { address: 'acme.research', org_id: remembered.id, parent_id: remembered.id });
const project: Project = { ...remembered, id: 'project', kind: 'project', name: 'Web', parent_id: remembered.id, topic_count: 1,
  path: [...remembered.path, { id: 'project', kind: 'project', name: 'Web', redacted: false }], access: { ...remembered.access, permissions: ['project.view'] } };
const topic: Topic = { ...remembered, id: 'topic', kind: 'topic', name: 'Sign in', parent_id: project.id, status: 'open', done_at: null, thread_display_state: null,
  path: [...project.path, { id: 'topic', kind: 'topic', name: 'Sign in', redacted: false }], access: { ...remembered.access, permissions: ['topic.view'] },
  thread_count: 0, attention: { running: 0, waiting_approval: 0, blocked: 0 }, work_attention: { uncommitted: 0, unpushed: 0, failed: 0 },
  environments: [], default_environment_id: null, last_activity_at: null, seq: 0 };
const clients: QueryClient[] = [];
function draw(ui: ReactNode = <Sidebar />, asAdmin = true) {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } }); clients.push(cache);
  cache.setQueryData(meKey, person); cache.setQueryData(orgsKey, [remembered]);
  cache.setQueryData(adminModeKey, { as_admin: asAdmin, available: true });
  cache.setQueryData(orgBySlugQuery(remembered.address).queryKey, remembered);
  cache.setQueryData(orgBySlugQuery(department.address).queryKey, department);
  cache.setQueryData(projectQuery(project.id).queryKey, project);
  cache.setQueryData(topicQuery(topic.id).queryKey, topic);
  useSelectedOrg.setState({ orgId: remembered.id });
  return show(<QueryClientProvider client={cache}>{ui}</QueryClientProvider>);
}
afterEach(async () => { cleanup(); for (const cache of clients.splice(0)) cache.clear(); useSelectedOrg.setState({ orgId: null }); useAppearance.setState(appearance); world.search = {}; vi.clearAllMocks(); await setLocale('en'); });

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  it(`keeps the chosen org navigation across local page sections at ${locale}/${theme}`, async () => {
    await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
    for (const [path, label] of [
      ['/settings/profile', i18n.t('settings.personal.nav'), '/settings/security'],
      ['/admin/overview', i18n.t('admin.nav.label'), '/admin/settings'],
      ['/help', i18n.t('workbench.areaNavigation.help.sections'), '/help?part=keys'],
      ['/usage', i18n.t('usagePerf.views'), '/usage?view=performance'],
    ] as const) {
      world.path = path; const view = draw();
      const nav = screen.getByRole('navigation', { name: i18n.t('views.navigation.title') });
      expect(within(nav).getByRole('button', { name: i18n.t('navigation.projects') })).toBeVisible();
      expect(screen.queryByRole('navigation', { name: label })).not.toBeInTheDocument();
      expect(world.tree).toHaveBeenCalledWith(remembered.id);
      expect(world.tree.mock.calls.every(([id]) => id === remembered.id)).toBe(true);
      view.unmount();
    }
  });
}

it('keeps a usage section’s scope, dates and filters in its actual destination', () => {
  world.path = '/usage'; world.search = { scope: 'node:00000000-0000-4000-8000-000000000001', range: 'last_week', model: 'selected-model', view: 'performance', show: 'speed' };
  draw(<AreaSections area="usage" />); const nav = screen.getByRole('navigation', { name: i18n.t('usagePerf.views') });
  const requests = within(nav).getByRole('link', { name: i18n.t('usagePerf.viewNames.requests') });
  const href = requests.getAttribute('href');
  if (href === null) throw new Error('The section has no native destination');
  const query = new URL(href, 'https://orch.test').searchParams;
  expect(query.get('scope')).toBe(world.search.scope); expect(query.get('range')).toBe('last_week');
  expect(query.get('model')).toBe('selected-model'); expect(query.get('view')).toBe('requests'); expect(query.has('show')).toBe(false);
  fireEvent.click(requests); expect(world.followed).toHaveBeenCalledWith(href);
});

it('retains the chosen org for page-local navigation, including articles and nested settings', () => {
  for (const path of ['/settings/profile', '/settings/security/recovery', '/admin/settings', '/help', '/help/approvals', '/usage']) {
    expect(navigationRoot(path, [remembered], remembered.id, { heading: remembered.id })).toBe(remembered.id);
  }
  expect(navigationRoot('/org/acme', [remembered], null, {})).toBe(remembered.org_id);
  expect(helpSearch({ part: 'keys' })).toEqual({ part: 'keys' });
  expect(helpSearch({ part: 'unknown' })).toEqual({});
});

function PlaceFrame({ scope, name }: { scope: PlaceSettingsNavigation; name: string }) {
  const [shown, setShown] = useState(true);
  return <><button onClick={() => setShown(!shown)}>Toggle navigation</button>
    <div hidden={!shown}><ViewOpeningContext.Provider value={shown ? 'settings-opening' : null}><Sidebar /></ViewOpeningContext.Provider></div>
    <SettingsFrame title="Settings section" label={i18n.t('settings.org.sections', { name })} current={scope.current} items={placeSettingsSections(scope, i18n.t)}>
      <input aria-label="Retained setting draft" defaultValue="" />
    </SettingsFrame>
  </>;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) it(`puts every place's settings in one visible navigation and retains the fallback draft at ${locale}/${theme}`, async () => {
  await setLocale(locale); useAppearance.setState({ theme, resolvedTheme: theme });
  const user = userEvent.setup();
  for (const [path, search, name] of [
    ['/org/acme/settings/policies', {}, remembered.name],
    ['/org/acme.research/settings/members', {}, department.name],
    ['/project/project/settings', { section: 'tags' }, project.name],
    ['/topic/topic', { tab: 'settings', section: 'policies' }, topic.name],
  ] as const) {
    world.path = path; world.search = search;
    const scope = settingsNavigation(path, search);
    if (scope === null || scope.kind === 'personal' || scope.kind === 'system') throw new Error('The place has no settings navigation');
    const view = draw(<PlaceFrame scope={scope} name={name} />);
    const label = i18n.t('settings.org.sections', { name });
    const nav = screen.getByRole('navigation', { name: label });
    expect(screen.getAllByRole('navigation', { name: label })).toHaveLength(1);
    expect(nav.closest('[data-settings-frame]')).not.toBeNull();
    expect(nav.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(nav.querySelector(`[data-section-id="${scope.current}"]`)).toHaveAttribute('aria-current', 'page');
    expect(screen.getByRole('navigation', { name: i18n.t('views.navigation.title') })).toBeVisible();
    expect(world.tree).toHaveBeenCalledWith(remembered.id);
    const draft = screen.getByRole('textbox', { name: 'Retained setting draft' });
    await user.type(draft, 'Unsaved words');
    await user.click(screen.getByRole('button', { name: 'Toggle navigation' }));
    const fallback = screen.getByRole('navigation', { name: label });
    expect(fallback.closest('[data-settings-frame]')).not.toBeNull();
    expect(fallback.querySelectorAll('[aria-current="page"]')).toHaveLength(1);
    expect(screen.getByRole('textbox', { name: 'Retained setting draft' })).toBe(draft);
    expect(draft).toHaveValue('Unsaved words');
    view.unmount();
  }
});

it('keeps Settings explicit, preserves exact setting links and org aliases, and lets plain topic links out', () => {
  expect(settingsNavigation('/topic/topic', {})).toBeNull();
  expect(settingsNavigation('/topic/topic', { tab: 'threads' })).toBeNull();
  expect(settingsNavigation('/topic/topic', { tab: 'settings', setting: 'agent.model' })).toEqual({ kind: 'topic', id: 'topic', current: 'policies' });
  expect(settingsNavigation('/project/project/settings', { setting: 'agent.model' })).toEqual({ kind: 'project', id: 'project', current: 'policies' });
  expect(settingsNavigation('/org/acme/settings/destinations')).toEqual({ kind: 'org', address: 'acme', current: 'channels' });
  expect(navigationRoot('/topic/topic', [remembered], remembered.id, { node: remembered.id })).toBe(remembered.id);
  expect(navigationRoot('/topic/topic', [remembered], remembered.id, { node: remembered.id })).toBe(remembered.id);
});
