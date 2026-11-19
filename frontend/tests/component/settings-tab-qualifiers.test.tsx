/** Actual settings title producers and frame lifetime; form internals/native answers are synthetic. */
import { act, cleanup, render, screen, waitFor, type RenderResult } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { AppWindow } from 'lucide-react';
import { useEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import type * as Live from '@/realtime/live';
import { ApiError, client } from '@/api/client';
import { nodeQuery, orgBySlugQuery, orgsQuery } from '@/api/nodes';
import { queryClient } from '@/api/query';
import type { Node, Org } from '@/generated/api-types';
import { OrgSettings } from '@/features/settings/org/OrgSettings';
import { ProjectSettings } from '@/features/settings/ScopeSettings';
import type { SettingsFrameProps } from '@/features/settings/Frame';
import { sectionLabel } from '@/features/settings/sectionLabels';
import { settingsNavigation } from '@/features/settings/navigation';
import { ViewHost, ViewSlot } from '@/features/workbench/Board';
import { allTabs, findTab, makeGroup, makeTab, type Tab } from '@/features/workbench/layout';
import { pageAddress, parsePageAddress } from '@/features/workbench/PageView';
import { loadWorkspace } from '@/features/workbench/persistence';
import { useWorkbench } from '@/features/workbench/store';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { TargetContext } from '@/shell/RouteContext';
import { clearScreenHeading, focusScreenHeading, forgetScreenHeading, useCurrentHeading, useRouteHeading } from '@/shell/screen';
import { treeWriter } from '@/realtime/stores/tree';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';
import { ACME } from '../doubles/fixtures';
import { World } from '../doubles/world';

const reads = vi.hoisted(() => ({ org: vi.fn<(slug: string) => Promise<Org>>(), node: vi.fn<(id: string) => Promise<Node>>(), thread: vi.fn(), document: vi.fn(), fetch: vi.fn<typeof fetch>() }));
vi.mock('@/api/nodes', async (load) => {
  const actual = await load<typeof Nodes>();
  return { ...actual,
    orgBySlugQuery: (slug: string) => ({ ...actual.orgBySlugQuery(slug), queryFn: () => reads.org(slug) }),
    nodeQuery: (id: string) => ({ ...actual.nodeQuery(id), queryFn: () => reads.node(id) }),
    projectQuery: (id: string) => ({ ...actual.projectQuery(id), queryFn: () => reads.node(id).then((row) => {
      if (row.kind !== 'project') throw new Error('The synthetic settings source must be a project'); return row;
    }) }),
  };
});
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: reads.thread, useDocumentFeed: reads.document }));
vi.mock('@/shell/org', () => ({ selectOrg: () => undefined }));
// Keep actual producers and their frame props, without creating/editing settings or mounting form readers.
vi.mock('@/features/settings/Frame', () => ({ SettingsFrame: ({ title, current }: SettingsFrameProps) => <section><h1>{title}</h1><output data-testid="body-section">{current}</output></section> }));

function nativeScene() {
  const world = new World(), person = world.people.get('ada');
  const root = world.places.rowOf(ACME.org), department = world.places.rowOf(ACME.research), project = world.places.rowOf(ACME.web), topic = world.places.rowOf(ACME.fixLogin);
  const tree = world.trees.get(ACME.org), shop = world.places.rowOf(ACME.shop);
  if (person === undefined || root === undefined || department === undefined || project === undefined || topic === undefined || tree === undefined || shop === undefined) throw new Error('The native-shaped title fixture requires its actual settings places');
  return { person, root: world.places.org(person, root), department: world.places.org(person, department), project: world.places.project(person, project), topic: world.places.topic(person, topic), secondProject: world.places.project(person, shop), tree: [...tree.nodes.values()] };
}
let scene: ReturnType<typeof nativeScene>;
const state = () => useWorkbench.getState(), owners = new Set<string>(), mounted: string[] = [], views: RenderResult[] = [], refused = new Set<string>();
const session = client.session.getState(), defaults = queryClient.getDefaultOptions();
let locale = currentLocale(), themeBefore = document.documentElement.dataset.theme, unregister: (() => void) | undefined;
function TopicCaption() {
  const heading = useCurrentHeading();
  return <output data-testid="topic-route-title">{heading?.title}</output>;
}
function Refusal() { useRouteHeading({ title: 'Current unavailable view', tabTitle: 'Current unavailable view', views: { ready: false } }); return <p>Current refusal</p>; }
function SettingsBody({ tab }: ViewProps) {
  const page = parsePageAddress(tab.of?.href ?? '');
  useEffect(() => { mounted.push(tab.id); }, [tab.id]);
  if (refused.has(tab.id)) return <Refusal />;
  if (page?.route === '/org/$slug/settings/$section/') return <OrgSettings slug={page.params.slug ?? ''} section={page.params.section ?? 'general'} />;
  if (page?.route === '/project/$projectId/settings/') {
    const navigation = settingsNavigation(page.pathname, page.search);
    return <ProjectSettings projectId={page.params.projectId ?? ''} section={navigation?.kind === 'project' ? navigation.current : 'general'} />;
  }
  if (page?.route === '/topic/$topicId/') return <><TargetContext target={page} /><TopicCaption /></>;
  return null;
}
function Frame() {
  const layout = useWorkbench((value) => value.layout), active = useWorkbench((value) => value.focusedMainTab), names = useWorkbench((value) => value.named);
  const tab = active === null ? null : findTab(layout, active)?.tab;
  return <FrameReadiness.Provider value={{ ready: true, pending: null }}><ViewHost layout={layout} />{tab !== null && tab !== undefined && <ViewSlot tab={tab} />}
    {allTabs(layout).filter((one) => one.view === 'page').map((one) => <output key={one.id} data-testid={`name-${one.id}`}>{names.get(one.id)?.title}</output>)}
  </FrameReadiness.Provider>;
}
function draw() {
  const view = render(<QueryClientProvider client={queryClient}><TooltipProvider><Frame /></TooltipProvider></QueryClientProvider>); views.push(view); return view;
}
function tab(href: string) { const one = makeTab('page', { href }); owners.add(one.id); return one; }
function seed(tabs: Tab[], selected = tabs[0]!) {
  state().enter(scene.person.me.id, 'page');
  state().change((layout) => ({ ...layout, main: makeGroup(tabs, selected.id), left: { ...layout.left, open: false, active: null, containers: [] }, right: { ...layout.right, open: false, active: null, containers: [] }, bottom: { ...layout.bottom, open: false, group: makeGroup() } })); state().focus(selected.id);
}
function putNative() {
  queryClient.setQueryData(orgsQuery.queryKey, [scene.root]);
  queryClient.setQueryData(orgBySlugQuery(scene.root.address).queryKey, scene.root);
  queryClient.setQueryData(orgBySlugQuery(scene.department.address).queryKey, scene.department);
  queryClient.setQueryData(nodeQuery(scene.project.id).queryKey, scene.project);
  queryClient.setQueryData(nodeQuery(scene.topic.id).queryKey, scene.topic);
  treeWriter.reset(scene.root.id, scene.tree, 1);
}
function forgetHeads() { clearViewOpenings(); focusScreenHeading(null); clearScreenHeading(); for (const id of owners) forgetScreenHeading(id); }
beforeEach(async () => {
  locale = currentLocale(); themeBefore = document.documentElement.dataset.theme; await setLocale('en'); state().leave(); localStorage.clear(); queryClient.clear(); treeWriter.clear(); forgetHeads(); mounted.length = 0; refused.clear();
  scene = nativeScene(); client.session.setState({ status: 'signed_in', userId: scene.person.me.id });
  queryClient.setDefaultOptions({ ...defaults, queries: { ...defaults.queries, retry: false, staleTime: Infinity } });
  reads.org.mockReset().mockRejectedValue(new Error('No unrequested org read')); reads.node.mockReset().mockRejectedValue(new Error('No unrequested node read'));
  reads.thread.mockClear(); reads.document.mockClear(); reads.fetch.mockReset().mockRejectedValue(new Error('No additional native title request')); vi.stubGlobal('fetch', reads.fetch);
  unregister = registerView({ name: 'page', title: 'views.screen.title', about: 'views.screen.about', icon: AppWindow, role: 'content', area: 'main', screens: 'all', body: SettingsBody });
});
afterEach(async () => {
  for (const view of views.splice(0)) view.unmount(); cleanup(); unregister?.(); unregister = undefined;
  state().leave(); localStorage.clear(); await queryClient.cancelQueries(); queryClient.clear(); queryClient.setDefaultOptions(defaults); treeWriter.clear(); forgetHeads(); owners.clear(); refused.clear();
  client.session.setState(session); vi.unstubAllGlobals();
  if (themeBefore === undefined) delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = themeBefore; await setLocale(locale);
});

it.each([['en', 'light'], ['en', 'dark'], ['zh-CN', 'light'], ['zh-CN', 'dark']] as const)('keeps distinct settings identity, exact body section and own URL with existing metadata only (%s/%s)', async (language, theme) => {
  await setLocale(language); document.documentElement.dataset.theme = theme; putNative();
  const general = sectionLabel('general', i18n.t);
  const tabs = [tab(`/org/${scene.root.address}/settings/general`), tab(`/org/${scene.department.address}/settings/general`), tab(`/project/${scene.project.id}/settings`), tab(`/topic/${scene.topic.id}?tab=settings`), tab(`/project/${scene.project.id}/settings`)];
  const expected = ['Acme', 'Research', 'Web', 'Fix login', 'Web'].map((name) => `${general} · ${name}`), hrefs = tabs.map(pageAddress);
  seed(tabs); draw();
  for (let index = 0; index < tabs.length; index += 1) {
    act(() => state().focus(tabs[index]!.id));
    await waitFor(() => expect(state().named.get(tabs[index]!.id)?.title).toBe(expected[index]));
    if (index !== 3) expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(general);
    else expect(screen.getByTestId('topic-route-title')).toHaveTextContent(i18n.t('screens.sections.settings'));
  }
  expect(tabs.map((one) => pageAddress(findTab(state().layout, one.id)!.tab))).toEqual(hrefs);
  expect(state().named.get(tabs[2]!.id)?.title).toBe(state().named.get(tabs[4]!.id)?.title);
  expect(reads.org).not.toHaveBeenCalled(); expect(reads.node).not.toHaveBeenCalled(); expect(reads.fetch).not.toHaveBeenCalled(); expect(reads.thread).not.toHaveBeenCalled(); expect(reads.document).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('distinguishes General, Defaults and Audit for the same native place without changing the section UI (%s)', async (language) => {
  await setLocale(language); putNative();
  const tabs = [tab(`/project/${scene.project.id}/settings`), tab(`/project/${scene.project.id}/settings?section=policies`), tab(`/project/${scene.project.id}/settings?section=audit`), tab(`/topic/${scene.topic.id}?tab=settings&section=policies`), tab(`/topic/${scene.topic.id}?tab=settings&setting=agent.model`)];
  const sections = ['general', 'policies', 'audit', 'policies', 'policies']; seed(tabs); draw();
  for (let index = 0; index < tabs.length; index += 1) {
    act(() => state().focus(tabs[index]!.id)); const label = sectionLabel(sections[index]!, i18n.t), place = index < 3 ? 'Web' : 'Fix login';
    await waitFor(() => expect(state().named.get(tabs[index]!.id)?.title).toBe(`${label} · ${place}`));
    if (index < 3) expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent(label);
  }
  expect(reads.node).not.toHaveBeenCalled(); expect(reads.fetch).not.toHaveBeenCalled();
});

it('retains a qualified cached name during a genuine cold settings read and focus/reload without opening dormant settings', async () => {
  putNative(); const first = tab(`/org/${scene.root.address}/settings/general`), next = tab(`/project/${scene.project.id}/settings`), dormant = tab('/org/unopened/settings/general'); seed([first, next, dormant]); draw();
  await waitFor(() => expect(state().named.get(first.id)?.title).toBe('General · Acme'));
  act(() => state().focus(next.id)); await waitFor(() => expect(state().named.get(next.id)?.title).toBe('General · Web'));
  act(() => state().focus(first.id)); await waitFor(() => expect(state().named.get(first.id)?.source).toBe('view'));
  await act(() => window.dispatchEvent(new Event('pagehide'))); const saved = loadWorkspace(scene.person.me.id, 'page').kept;
  expect(saved?.titles?.[first.id]?.title).toBe('General · Acme');
  const before = allTabs(state().layout).map((one) => ({ id: one.id, href: pageAddress(one) }));
  for (const view of views.splice(0)) view.unmount(); state().leave(); forgetHeads(); queryClient.clear(); mounted.length = 0;
  let finish: ((org: Org) => void) | undefined; reads.org.mockImplementation((slug) => {
    expect(slug).toBe(scene.root.address); return new Promise<Org>((resolve) => { finish = resolve; });
  });
  queryClient.setQueryData(nodeQuery(scene.project.id).queryKey, scene.project);
  state().enter(scene.person.me.id, 'page'); draw();
  await waitFor(() => expect(reads.org).toHaveBeenCalledExactlyOnceWith(scene.root.address));
  expect(state().named.get(first.id)?.title).toBe('General · Acme'); expect(state().named.get(first.id)?.source).toBe('cache');
  act(() => state().focus(next.id)); await waitFor(() => expect(state().named.get(next.id)?.title).toBe('General · Web'));
  await act(async () => { if (finish === undefined) throw new Error('The original native org read must be pending'); finish(scene.root); await Promise.resolve(); });
  expect(state().named.get(first.id)?.title).toBe('General · Acme');
  act(() => state().focus(first.id)); await waitFor(() => expect(state().named.get(first.id)?.source).toBe('view'));
  expect(allTabs(state().layout).map((one) => ({ id: one.id, href: pageAddress(one) }))).toEqual(before);
  expect(new Set(mounted)).toEqual(new Set([first.id, next.id])); expect(mounted).not.toContain(dormant.id);
  expect(reads.org).toHaveBeenCalledTimes(1); expect(reads.node).not.toHaveBeenCalled(); expect(reads.fetch).not.toHaveBeenCalled();
});

it('does not restore hidden ancestor names from tree memory into a redacted current path or tab tooltip', async () => {
  putNative(); const project = { ...scene.project, path: scene.project.path.map((part) => part.id === scene.project.id ? part : { ...part, name: 'Private ancestor', redacted: true }) };
  queryClient.setQueryData(nodeQuery(project.id).queryKey, project);
  const own = tab(`/project/${project.id}/settings`); seed([own]); draw();
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('General · Web'));
  expect(state().named.get(own.id)?.title).not.toMatch(/Acme|Research|Private ancestor/);
  expect(state().named.get(own.id)?.path).toBe('… / … / Web'); expect(reads.node).not.toHaveBeenCalled(); expect(reads.fetch).not.toHaveBeenCalled();
});

it('replaces a former qualified title and path after an actual query refusal even while stale data remains cached', async () => {
  putNative(); const own = tab(`/project/${scene.project.id}/settings`); seed([own]); draw();
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('General · Web'));
  const query = queryClient.getQueryCache().find({ queryKey: nodeQuery(scene.project.id).queryKey });
  if (query === undefined) throw new Error('The real project query must exist before its refusal');
  const failure = new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: 'Unavailable', detail: 'The synthetic person no longer reads this project', status: 404, request_id: 'settings-refused' }, null);
  act(() => query.setState({ status: 'error', fetchStatus: 'idle', error: failure }));
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('General'));
  expect(query.state.data).toBe(scene.project); expect(state().named.get(own.id)?.path).toBeUndefined(); expect(state().named.get(own.id)?.source).toBe('view');
  expect(reads.fetch).not.toHaveBeenCalled();
});

it('lets a current explicit refusal heading win over restored names and tree metadata', async () => {
  putNative(); const own = tab(`/org/${scene.root.address}/settings/general`); seed([own]); const view = draw();
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('General · Acme'));
  refused.add(own.id); view.rerender(<QueryClientProvider client={queryClient}><TooltipProvider><Frame /></TooltipProvider></QueryClientProvider>);
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('Current unavailable view'));
  act(() => treeWriter.reset(scene.root.id, scene.tree, 2));
  expect(state().named.get(own.id)?.title).toBe('Current unavailable view'); expect(state().named.get(own.id)?.path).toBeUndefined();
});

it('leaves another person no qualified title snapshot from the previous workspace', async () => {
  putNative(); const own = tab(`/org/${scene.root.address}/settings/general`); seed([own]); const view = draw();
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('General · Acme'));
  state().flush(); view.unmount(); views.splice(views.indexOf(view), 1); state().leave(); forgetHeads(); queryClient.clear(); treeWriter.clear();
  client.session.setState({ status: 'signed_in', userId: 'different-settings-reader' }); state().enter('different-settings-reader', 'page');
  expect(state().named.size).toBe(0); expect(allTabs(state().layout).some((one) => one.id === own.id)).toBe(false);
  expect(reads.fetch).not.toHaveBeenCalled();
});

it('qualifies equal project names from the already loaded tree with the shortest distinct parent', async () => {
  putNative();
  const other = { ...scene.secondProject, name: 'Web', path: scene.secondProject.path.map((part) => part.id === scene.secondProject.id ? { ...part, name: 'Web' } : part) };
  queryClient.setQueryData(nodeQuery(other.id).queryKey, other);
  treeWriter.reset(scene.root.id, scene.tree.map((row) => row.id === other.id ? { ...row, name: 'Web' } : row), 2);
  const first = tab(`/project/${scene.project.id}/settings`), second = tab(`/project/${other.id}/settings`); seed([first, second]); draw();
  await waitFor(() => expect(state().named.get(first.id)?.title).toBe('General · Web · Research'));
  act(() => state().focus(second.id)); await waitFor(() => expect(state().named.get(second.id)?.title).toBe('General · Web · Acme'));
  expect(state().named.get(first.id)?.title).not.toBe(state().named.get(second.id)?.title);
  expect(reads.node).not.toHaveBeenCalled(); expect(reads.fetch).not.toHaveBeenCalled();
});

it('requalifies a visible same-named root from a later authorized cache publication without fetching either org', async () => {
  putNative(); const own = tab(`/org/${scene.root.address}/settings/general`); seed([own]); draw();
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe('General · Acme'));
  const other: Org = { ...scene.root, id: '00000000-0000-7000-8000-000000000991', org_id: '00000000-0000-7000-8000-000000000991', slug: 'acme-eu', address: 'acme-eu', path: [{ id: '00000000-0000-7000-8000-000000000991', kind: 'org', name: 'Acme', redacted: false }] };
  await act(() => queryClient.setQueryData(orgsQuery.queryKey, [scene.root, other]));
  await waitFor(() => expect(state().named.get(own.id)?.title).toBe(`General · Acme · ${scene.root.address}`));
  expect(reads.org).not.toHaveBeenCalled(); expect(reads.node).not.toHaveBeenCalled(); expect(reads.fetch).not.toHaveBeenCalled();
});
