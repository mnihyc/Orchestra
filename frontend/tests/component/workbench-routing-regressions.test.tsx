/** Real installed router + real store/PageRouting/PageView/Board. Only native reads and feature bodies are synthetic. */
import { act, cleanup, fireEvent, render, screen, waitFor, within, type RenderResult } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, type AnyRoute } from '@tanstack/react-router';
import { PanelsTopLeft } from 'lucide-react';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import type * as Nodes from '@/api/nodes';
import type * as Live from '@/realtime/live';
import { ApiError, client } from '@/api/client';
import { queryClient } from '@/api/query';
import { commandsNow } from '@/features/finder/commands';
import { addMainTab, allTabs, applyGrid, findTab, makeContainer, makeGroup, makeTab, newId, spatialMainGroups, type MainNode, type Tab } from '@/features/workbench/layout';
import { panelRead, panelTarget } from '@/features/workbench/panelScope';
import { clearPageNavigation, markPageNavigation, pageNavigationOrigin, usePageFailures } from '@/features/workbench/pageNavigation';
import { openPage, pageAddress, PageView } from '@/features/workbench/PageView';
import { loadWorkspace } from '@/features/workbench/persistence';
import { registerShippedViews } from '@/features/workbench/shippedViews';
import { useWorkbench } from '@/features/workbench/store';
import { retireThreadWindows } from '@/features/workbench/threadWindows';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { registerView } from '@/features/workbench/views';
import { workspaceEntries } from '@/features/workbench/workspaceActions';
import { i18n } from '@/i18n';
import { Route as ThreadRoute } from '@/routes/thread/$threadId/index';
import { Route as OrgRoute } from '@/routes/org/$slug/index';
import { Route as OrgSettingsRoute } from '@/routes/org/$slug/settings/$section/index';
import { Route as AdminRoute } from '@/routes/admin/$section/index';
import { clearScreenHeading, focusScreenHeading } from '@/shell/screen';
import { TooltipProvider } from '@/ui/Tooltip';
import { RoutingFrame } from './workbench-routing/frame';
import {
  bodies, BottomInspector, canonicalCalls, capturedNavigate, department, holdCanonical,
  metadataDemands, ORG_ADDRESS, ORG_ID, releasePendingCanonical, resetFeatureEvidence, USER,
} from './workbench-routing/fixture';

vi.mock('@/api/nodes', async (original) => {
  const actual = await original<typeof Nodes>();
  const { readCanonicalNode } = await import('./workbench-routing/fixture');
  return { ...actual, nodeQuery: (id: string) => ({ ...actual.nodeQuery(id), queryFn: () => readCanonicalNode(id) }) };
});
vi.mock('@/features/thread/Thread', async () => {
  const { ThreadBody } = await import('./workbench-routing/fixture');
  return { ThreadScreen: ThreadBody };
});
vi.mock('@/features/org/Home', async () => {
  const { OrgBody } = await import('./workbench-routing/fixture');
  return { OrgHome: OrgBody };
});
vi.mock('@/features/settings/org/OrgSettings', async () => {
  const { OrgSettingsBody } = await import('./workbench-routing/fixture');
  return { OrgSettings: OrgSettingsBody };
});
vi.mock('@/features/admin/Management', async () => {
  const { ManagementBody } = await import('./workbench-routing/fixture');
  return { Management: ManagementBody };
});
vi.mock('@/realtime/live', async (original) => {
  const actual = await original<typeof Live>();
  const { useSyntheticThreadFeed } = await import('./workbench-routing/fixture');
  return { ...actual, useThreadFeed: useSyntheticThreadFeed };
});

const state = () => useWorkbench.getState();
const originalSession = client.session.getState();
const originalDefaults = queryClient.getDefaultOptions();
const unregister: (() => void)[] = [];
const mounted: RenderResult[] = [];
const histories: ReturnType<typeof createMemoryHistory>[] = [];

beforeAll(() => { registerShippedViews(() => null); });
beforeEach(() => {
  state().leave(); localStorage.clear(); queryClient.clear(); resetFeatureEvidence(); clearViewOpenings();
  clearPageNavigation(pageNavigationOrigin()); usePageFailures.setState({ byTab: new Map() });
  focusScreenHeading(null); clearScreenHeading();
  client.session.setState({ status: 'signed_in', userId: USER });
  queryClient.setDefaultOptions({ ...originalDefaults, queries: { ...originalDefaults.queries, retry: false } });
  for (const name of ['screen', 'page'] as const) unregister.push(registerView({ name,
    title: 'views.screen.title', about: 'views.screen.about', icon: PanelsTopLeft,
    screens: 'all', area: 'main', role: 'content', body: PageView }));
  unregister.push(registerView({ name: 'navigation', title: 'views.navigation.title', about: 'views.navigation.about', icon: PanelsTopLeft,
    screens: 'all', area: 'left', body: () => null }));
  unregister.push(registerView({ name: 'resource-navigation', title: 'navigation.resources.title', about: 'navigation.resources.about', icon: PanelsTopLeft,
    screens: 'all', area: 'left', body: () => null }));
  unregister.push(registerView({ name: 'terminal', title: 'views.terminal.title', about: 'views.terminal.about', icon: PanelsTopLeft,
    screens: 'all', area: 'bottom', role: 'inspector', body: BottomInspector }));
  for (const name of ['files', 'changes'] as const) unregister.push(registerView({ name,
    title: name === 'files' ? 'views.files.title' : 'views.changes.title',
    about: name === 'files' ? 'views.files.about' : 'views.changes.about',
    icon: PanelsTopLeft, screens: 'all', area: 'right', role: 'inspector', body: () => null }));
});
afterEach(async () => {
  // End admission before leaving, so the still-mounted bridge cannot restore this user again.
  const before = state().layout;
  act(() => {
    client.session.setState({ status: 'signed_out', userId: null });
    clearPageNavigation(pageNavigationOrigin()); state().leave();
    retireThreadWindows(before, state().layout);
  });
  for (const view of mounted.splice(0)) view.unmount();
  cleanup();
  for (const history of histories.splice(0)) history.destroy();
  await act(async () => { releasePendingCanonical(); await queryClient.cancelQueries(); });
  queryClient.clear(); queryClient.setDefaultOptions(originalDefaults);
  for (const remove of unregister.splice(0)) remove();
  clearPageNavigation(pageNavigationOrigin()); usePageFailures.setState({ byTab: new Map() }); clearViewOpenings();
  focusScreenHeading(null); clearScreenHeading();
  client.session.setState(originalSession); localStorage.clear();
});

function address(id: string, version?: string): string {
  return `/thread/${id}${version === undefined ? '' : `?continuation=${version}`}`;
}
function page(href: string): Tab { return makeTab('page', { href }); }
function requiredHref(tab: Tab): string {
  const href = tab.of?.href;
  if (href === undefined) throw new Error('An addressed page fixture lost its actual URL.');
  return href;
}
function pageTabs(): Tab[] { return allTabs(state().layout).filter((tab) => tab.view === 'page'); }
function ids(): string[] { return pageTabs().map((tab) => tab.id); }
function hrefOf(id: string): string | null {
  const place = findTab(state().layout, id);
  return place === null ? null : pageAddress(place.tab);
}
function body(id: string): HTMLElement { return screen.getByTestId(`body-${id}`); }
function ownedView(id: string): HTMLElement {
  const element = document.querySelector(`[data-view-instance="${id}"]`);
  if (!(element instanceof HTMLElement)) throw new Error(`No visible workbench owner ${id}`);
  return element;
}
function localInput(id: string): HTMLElement { return within(body(id)).getByRole('textbox', { name: 'Local view state' }); }
function bodyToken(id: string): string {
  const token = body(id).getAttribute('data-instance');
  if (token === null) throw new Error('The feature body did not expose its mounted identity.');
  return token;
}
function openOwned(href: string, mode: 'normal' | 'new' | 'beside' = 'normal'): string {
  const written: { id: string | null } = { id: null };
  act(() => { written.id = openPage(href, mode); });
  if (written.id === null) throw new Error(`The internal destination did not open: ${href}`);
  return written.id;
}

function seed(tabs: Tab[], selected: string, split = false, withBottom = false) {
  state().enter(USER, 'thread');
  const groups = split ? tabs.map((tab) => makeGroup([tab])) : [makeGroup(tabs, selected)];
  const inspector = makeTab('terminal');
  const main: MainNode = split
    ? { kind: 'split', id: newId('s'), direction: 'row', sizes: groups.map(() => 100 / groups.length), children: groups }
    : groups[0]!;
  state().change((layout) => ({ ...layout, main, maximized: null,
    left: { ...layout.left, open: false, containers: [], active: null },
    right: { ...layout.right, open: false, containers: [], active: null },
    bottom: { ...layout.bottom, open: withBottom, group: makeGroup(withBottom ? [inspector] : []) },
  }));
  state().focus(selected); state().flush();
  return { groups, inspector };
}

/** Configure actual file-route declarations as the generated tree does, without mocking their hooks or loaders. */
function draw(href: string) {
  const root = createRootRoute({ component: Outlet, shellComponent: RoutingFrame,
    errorComponent: () => <p>Outer route error must be handed to its workbench owner</p>,
    notFoundComponent: () => <p>Outer NotFound must be handed to its workbench owner</p> });
  const bind = (route: AnyRoute, path: string) => { Object.assign(route.options, { id: path, path, getParentRoute: () => root }); };
  bind(ThreadRoute, '/thread/$threadId/'); bind(OrgRoute, '/org/$slug/');
  bind(OrgSettingsRoute, '/org/$slug/settings/$section/'); bind(AdminRoute, '/admin/$section/');
  const neutral = createRoute({ getParentRoute: () => root, path: '/workbench', staticData: { screen: 'page' }, component: () => null });
  const routeTree = root.addChildren([ThreadRoute, OrgRoute, OrgSettingsRoute, AdminRoute, neutral]);
  const history = createMemoryHistory({ initialEntries: [href] }); histories.push(history);
  const router = createRouter({ routeTree, history, defaultPendingMs: 0, defaultPendingMinMs: 0, defaultPreload: false });
  const view = render(<QueryClientProvider client={queryClient}><TooltipProvider><RouterProvider router={router} /></TooltipProvider></QueryClientProvider>);
  mounted.push(view);
  return { router, history, view };
}
type Drawn = ReturnType<typeof draw>;
async function settled(drawn: Drawn, href?: string): Promise<void> {
  await waitFor(() => {
    expect(drawn.router.state.isLoading).toBe(false);
    expect(drawn.router.state.status).toBe('idle');
    if (href !== undefined) expect(drawn.router.state.location.href).toBe(href);
  });
}
async function focusBottom(): Promise<void> {
  const control = await screen.findByRole('button', { name: 'Inspector focus target' });
  act(() => { control.focus(); });
  expect(state().focusedTab).toBe(state().layout.bottom.group.active);
}
function runLayout(id: string): void {
  const entry = workspaceEntries(i18n.t).find((candidate) => candidate.id === id);
  if (entry === undefined || !('onSelect' in entry)) throw new Error(`No actionable Layout entry ${id}`);
  entry.onSelect();
}
function runPalette(id: string): void {
  const command = commandsNow().find((candidate) => candidate.id === id);
  if (command === undefined) throw new Error(`No registered workspace command ${id}`);
  command.run('here');
}

describe('duplicate instances and owned URL changes', () => {
  it.each(['new', 'beside'] as const)('the actual %s opener gives a duplicate its own body and ordinary reuse prefers that active owner', async (mode) => {
    const a = page(address('same-thread', 'version-x'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const tokenA = bodyToken(a.id);
    fireEvent.change(localInput(a.id), { target: { value: 'Original owner state' } });
    const duplicate = openOwned(requiredHref(a), mode);
    expect(duplicate).not.toBe(a.id);
    await screen.findByTestId(`body-${duplicate}`); await settled(drawn, requiredHref(a));
    expect(body(duplicate)).toHaveAttribute('data-read-owner', duplicate); expect(bodyToken(duplicate)).not.toBe(tokenA);
    const reused = openOwned(address('same-thread', 'version-z'));
    expect(reused).toBe(duplicate);
    await settled(drawn, address('same-thread', 'version-z'));
    expect(ids()).toEqual([a.id, duplicate]); expect(hrefOf(a.id)).toBe(requiredHref(a));
    expect(hrefOf(duplicate)).toBe(address('same-thread', 'version-z')); expect(state().activeContent).toBe(duplicate);
    act(() => { state().focus(a.id); }); await settled(drawn, requiredHref(a));
    expect(bodyToken(a.id)).toBe(tokenA); expect(localInput(a.id)).toHaveValue('Original owner state');
  });

  it('reload preserves the exact selected duplicate, locators and independently keyed body', async () => {
    const a = page(address('same-thread', 'version-x')), b = page(address('same-thread', 'version-y'));
    seed([a, b], b.id, true); state().leave();
    const drawn = draw(requiredHref(b));
    await settled(drawn, requiredHref(b));
    await Promise.all([screen.findByTestId(`body-${a.id}`), screen.findByTestId(`body-${b.id}`)]);
    expect(ids()).toEqual([a.id, b.id]);
    expect(state().focusedTab).toBe(b.id); expect(state().activeContent).toBe(b.id);
    expect(hrefOf(a.id)).toBe(requiredHref(a)); expect(hrefOf(b.id)).toBe(requiredHref(b));
    expect(body(a.id)).toHaveAttribute('data-continuation', 'version-x');
    expect(body(b.id)).toHaveAttribute('data-continuation', 'version-y');
    expect(body(a.id)).toHaveAttribute('data-read-owner', a.id); expect(body(b.id)).toHaveAttribute('data-read-owner', b.id);
    expect(bodyToken(a.id)).not.toBe(bodyToken(b.id));
  });

  it('a cold reload retains main-inspector focus and names the remembered content without mounting its hidden body', async () => {
    const a = page(address('same-thread', 'version-x')), b = page(address('same-thread', 'version-y'));
    const { groups } = seed([a, b], b.id);
    const inspector = makeTab('terminal');
    state().change((layout) => addMainTab(layout, inspector, groups[0]!.id));
    state().focus(inspector.id); state().leave(); clearViewOpenings();
    const drawn = draw(requiredHref(b));
    await settled(drawn, requiredHref(b));
    expect(await screen.findByRole('button', { name: 'Inspector focus target' })).toBeVisible();
    await waitFor(() => { expect(screen.getByTestId('focused-caption')).toHaveTextContent('Thread same-thread'); });
    expect(state().focusedTab).toBe(inspector.id); expect(state().focusedMainTab).toBe(inspector.id); expect(state().activeContent).toBe(b.id);
    expect(ids()).toEqual([a.id, b.id]); expect(hrefOf(a.id)).toBe(requiredHref(a)); expect(hrefOf(b.id)).toBe(requiredHref(b));
    // The metadata supplier may mount; neither conversation body was given a visible slot.
    expect(bodies.filter((entry) => entry.kind === 'thread')).toEqual([]);
    expect(metadataDemands.some((entry) => entry.owner === b.id && entry.threadId === 'same-thread')).toBe(true);
    expect(metadataDemands.every((entry) => !entry.reportVisible)).toBe(true);
  });

  it('a focused duplicate changes version through its real route controller without rewriting the first copy', async () => {
    const a = page(address('same-thread', 'version-x')), b = page(address('same-thread', 'version-y'));
    seed([a, b], b.id, true);
    const drawn = draw(requiredHref(b));
    await Promise.all([screen.findByTestId(`body-${a.id}`), screen.findByTestId(`body-${b.id}`)]); await settled(drawn, requiredHref(b));
    const tokenA = bodyToken(a.id), tokenB = bodyToken(b.id);
    fireEvent.change(localInput(a.id), { target: { value: 'A local expansion marker' } });
    fireEvent.change(localInput(b.id), { target: { value: 'B local expansion marker' } });
    fireEvent.click(within(body(b.id)).getByRole('button', { name: 'Choose version Z' }));
    await settled(drawn, address('same-thread', 'version-z'));
    await waitFor(() => { expect(body(b.id)).toHaveAttribute('data-continuation', 'version-z'); });
    expect(ids()).toEqual([a.id, b.id]); expect(state().activeContent).toBe(b.id); expect(state().focusedTab).toBe(b.id);
    expect(hrefOf(a.id)).toBe(requiredHref(a)); expect(hrefOf(b.id)).toBe(address('same-thread', 'version-z'));
    expect(body(a.id)).toHaveAttribute('data-continuation', 'version-x');
    expect(bodyToken(a.id)).toBe(tokenA); expect(bodyToken(b.id)).toBe(tokenB);
    expect(localInput(a.id)).toHaveValue('A local expansion marker'); expect(localInput(b.id)).toHaveValue('B local expansion marker');
    expect(pageNavigationOrigin()).toBeNull();
  });

  it('a still-visible unfocused owner updates only its own search, without navigating the focused duplicate', async () => {
    const a = page(address('same-thread', 'version-x')), b = page(address('same-thread', 'version-y'));
    seed([a, b], b.id, true);
    const drawn = draw(requiredHref(b));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(b));
    await act(async () => { await capturedNavigate(a.id, { search: { continuation: 'version-z' } }); });
    expect(hrefOf(a.id)).toBe(address('same-thread', 'version-z')); expect(hrefOf(b.id)).toBe(requiredHref(b));
    expect(state().activeContent).toBe(b.id); expect(state().focusedTab).toBe(b.id);
    expect(drawn.router.state.location.href).toBe(requiredHref(b)); expect(ids()).toEqual([a.id, b.id]);
  });

  it('Back and Forward focus existing resources and preserve the other instance and its local state', async () => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const token = bodyToken(a.id);
    fireEvent.change(localInput(a.id), { target: { value: 'A survives history traversal' } });
    await act(async () => { await drawn.router.navigate({ href: address('b') }); });
    await settled(drawn, address('b'));
    const b = pageTabs().find((tab) => tab.id !== a.id);
    if (b === undefined) throw new Error('Ordinary new-resource navigation did not open B.');
    expect(drawn.history.length).toBe(2);
    act(() => { drawn.history.back(); }); await settled(drawn, address('a'));
    expect(state().activeContent).toBe(a.id); expect(ids()).toEqual([a.id, b.id]);
    expect(bodyToken(a.id)).toBe(token); expect(localInput(a.id)).toHaveValue('A survives history traversal');
    act(() => { drawn.history.forward(); }); await settled(drawn, address('b'));
    expect(state().activeContent).toBe(b.id); expect(ids()).toEqual([a.id, b.id]);
  });

  it('Close all uses the neutral route, survives reload empty and reopens the exact last closed locator', async () => {
    const a = page(address('a', 'version-x')), b = page(address('b', 'version-y'));
    seed([a, b], b.id, true);
    const first = draw(requiredHref(b));
    await screen.findByTestId(`body-${b.id}`); await settled(first, requiredHref(b));
    act(() => { runLayout('close-all-main'); });
    await settled(first, '/workbench');
    expect(ids()).toEqual([]); expect(state().activeContent).toBeNull();
    expect(state().closedTabs.map((entry) => entry.tab.id)).toEqual([a.id, b.id]);
    first.view.unmount(); first.history.destroy();
    state().leave(); clearViewOpenings(); resetFeatureEvidence();
    const second = draw('/workbench');
    await settled(second, '/workbench');
    expect(ids()).toEqual([]); expect(bodies).toEqual([]);
    act(() => { runLayout('reopen'); });
    await settled(second, requiredHref(b));
    expect(ids()).toEqual([b.id]); expect(hrefOf(b.id)).toBe(requiredHref(b));
    expect(await screen.findByTestId(`body-${b.id}`)).toHaveAttribute('data-continuation', 'version-y');
  });
});

describe('main origin is independent from bottom keyboard focus', () => {
  it.each(['normal', 'new', 'beside'] as const)('opens %s from G2 after the actual bottom inspector receives focus', async (mode) => {
    const a = page(address('a')), b = page(address('b'));
    const { groups, inspector } = seed([a, b], b.id, true, true);
    const g1 = groups[0]!.id, g2 = groups[1]!.id;
    const drawn = draw(requiredHref(b));
    await settled(drawn, requiredHref(b)); await focusBottom();
    expect(state().focusedTab).toBe(inspector.id); expect(state().focusedMainGroup).toBe(g2); expect(state().focusedMainTab).toBe(b.id);
    const opened = openOwned(address('c'), mode);
    await settled(drawn, address('c'));
    const destination = findTab(state().layout, opened);
    if (destination === null) throw new Error('The new tab has no retained location.');
    if (mode === 'beside') {
      expect(destination.group.id).not.toBe(g1); expect(destination.group.id).not.toBe(g2);
      expect(spatialMainGroups(state().layout.main).map((group) => group.id)).toEqual([g1, g2, destination.group.id]);
    } else expect(destination.group.id).toBe(g2);
    expect(ids()).toEqual([a.id, b.id, opened]);
    expect(findTab(state().layout, inspector.id)?.area).toBe('bottom');
  });

  it.each(['layout', 'palette'] as const)('%s Close other main tabs retains the remembered main tab and bottom inspector', async (surface) => {
    const a = page(address('a')), b = page(address('b'));
    const { inspector } = seed([a, b], b.id, true, true);
    const drawn = draw(requiredHref(b));
    await settled(drawn, requiredHref(b)); await focusBottom();
    act(() => { if (surface === 'layout') runLayout('close-other-main'); else runPalette('view.close-other-main'); });
    expect(ids()).toEqual([b.id]); expect(findTab(state().layout, a.id)).toBeNull();
    expect(findTab(state().layout, inspector.id)?.area).toBe('bottom');
    expect(state().focusedMainTab).toBe(b.id); expect(state().activeContent).toBe(b.id);
    expect(drawn.router.state.location.href).toBe(requiredHref(b));
  });

  it('retains an explicitly chosen empty grid insertion cell after bottom focus and across persistence', async () => {
    const a = page(address('a'));
    const { inspector } = seed([a], a.id, false, true);
    state().change((layout) => applyGrid(layout, '2x4', a.id));
    const empty = spatialMainGroups(state().layout.main)[5];
    if (empty === undefined) throw new Error('The explicit grid cell is missing.');
    state().setFocusedGroup(empty.id); state().focus(inspector.id); state().flush();
    expect(loadWorkspace(USER, 'thread').kept?.focusedMainGroup).toBe(empty.id);
    state().leave();
    const drawn = draw(requiredHref(a));
    await settled(drawn, requiredHref(a)); await focusBottom();
    expect(state().focusedMainGroup).toBe(empty.id); expect(state().focusedMainTab).toBeNull();
    const opened = openOwned(address('b'));
    expect(findTab(state().layout, opened)?.group.id).toBe(empty.id);
    expect(spatialMainGroups(state().layout.main)).toHaveLength(8);
  });

  it.each(['layout', 'palette'] as const)('%s grid keeps the overflow main selection while keyboard focus remains in the inspector', async (surface) => {
    const tabs = Array.from({ length: 10 }, (_, index) => page(address(`thread-${index}`)));
    const last = tabs[9]!;
    const { inspector } = seed(tabs, last.id, false, true);
    const drawn = draw(requiredHref(last));
    await settled(drawn, requiredHref(last)); await focusBottom();
    act(() => { if (surface === 'layout') runLayout('grid-2x4'); else runPalette('view.grid.2x4'); });
    const cells = spatialMainGroups(state().layout.main);
    expect(cells).toHaveLength(8); expect(cells[7]?.active).toBe(last.id);
    expect(cells[7]?.tabs.map((tab) => tab.id)).toEqual(tabs.slice(7).map((tab) => tab.id));
    expect(state().focusedTab).toBe(inspector.id); expect(state().focusedMainTab).toBe(last.id); expect(state().activeContent).toBe(last.id);
    expect(ids()).toEqual(tabs.map((tab) => tab.id)); expect(drawn.router.state.location.href).toBe(requiredHref(last));
  });
});

describe('frame recovery keeps the visible main selection and content context aligned', () => {
  it.each(['reset', 'conversation', 'code', 'review'] as const)('%s keeps B in front after bottom focus and preserves A local state/dirty identity', async (choice) => {
    const a = page(address('a')), b = page(address('b'));
    seed([a, b], a.id, false, true);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const token = bodyToken(a.id);
    fireEvent.change(localInput(a.id), { target: { value: 'Retain the actual mounted A state' } });
    act(() => { state().setUnsaved(a.id, true); state().focus(b.id); });
    await settled(drawn, requiredHref(b)); await focusBottom();
    act(() => { if (choice === 'reset') state().reset(); else state().choose(choice); });
    const main = spatialMainGroups(state().layout.main);
    expect(main).toHaveLength(1); expect(main[0]?.active).toBe(b.id);
    expect(state().focusedMainTab).toBe(b.id); expect(state().activeContent).toBe(b.id);
    expect(ids()).toEqual([a.id, b.id]); expect(state().unsaved.has(a.id)).toBe(true);
    await settled(drawn, requiredHref(b));
    expect(await screen.findByTestId(`body-${b.id}`)).toBeVisible();
    expect(screen.getByTestId('focused-caption')).toHaveTextContent('Thread b');
    act(() => { state().focus(a.id); }); await settled(drawn, requiredHref(a));
    expect(bodyToken(a.id)).toBe(token); expect(localInput(a.id)).toHaveValue('Retain the actual mounted A state');
  });
});

describe('sidebar movement carries presentation focus without changing content context', () => {
  function sidebar(tabs: Tab[], selected: string, split = false) {
    const seeded = seed(tabs, selected, split, true);
    for (const name of ['chat', 'outline'] as const) unregister.push(registerView({ name,
      title: name === 'chat' ? 'views.chat.title' : 'views.outline.title',
      about: name === 'chat' ? 'views.chat.about' : 'views.outline.about',
      icon: PanelsTopLeft, screens: 'all', area: 'right', role: 'inspector',
      body: () => <button type="button">Focus {name} panel</button>,
    }));
    const chat = makeTab('chat'), outline = makeTab('outline');
    const container = makeContainer('conversation', [chat, outline], chat.id);
    state().change((layout) => ({ ...layout, right: { ...layout.right, open: true, active: container.name,
      containers: [{ ...container, sections: container.sections.map((section) => ({ ...section, open: true })) }],
    } }));
    return { ...seeded, chat, outline };
  }

  async function moveChatFromHeader(chat: Tab, destination: 'main' | 'split') {
    const bar = document.querySelector('[data-workbench-bar="right"]');
    if (!(bar instanceof HTMLElement)) throw new Error('The actual right sidebar is missing.');
    const heading = within(bar).getByRole('button', { name: i18n.t('views.chat.title') });
    act(() => { heading.focus(); });
    expect(heading).toHaveFocus();
    // jsdom has no browser default for Shift+F10; dispatch the native contextmenu event it produces.
    fireEvent.contextMenu(heading);
    const move = await screen.findByRole('menuitem', { name: i18n.t('workbench.move.label') });
    fireEvent.keyDown(move, { key: 'ArrowRight' });
    const name = destination === 'main' ? i18n.t('workbench.choices.areas.main.name') : i18n.t('workbench.choices.split.right.name');
    fireEvent.click(await screen.findByRole('menuitem', { name }));
    await waitFor(() => {
      expect(findTab(state().layout, chat.id)?.area).toBe('main');
      expect(document.getElementById(`tab-${chat.id}`)).toHaveAttribute('aria-selected', 'true');
    });
  }

  async function focusOutline(outline: Tab) {
    const control = await screen.findByRole('button', { name: 'Focus outline panel' });
    act(() => { control.focus(); });
    expect(state().focusedTab).toBe(outline.id);
  }

  it.each(['main', 'split'] as const)('the actual header menu moves Chat to %s and reset retains its selection, target and live conversation state', async (destination) => {
    const a = page(address('a', 'version-x'));
    const { chat, outline } = sidebar([a], a.id);
    const drawn = draw(requiredHref(a));
    await settled(drawn, requiredHref(a)); await screen.findByTestId(`body-${a.id}`);
    const token = bodyToken(a.id);
    fireEvent.change(localInput(a.id), { target: { value: 'Keep the conversation state while Chat is selected' } });
    act(() => { state().setUnsaved(a.id, true); });
    await moveChatFromHeader(chat, destination);
    expect(state().focusedTab).toBe(chat.id); expect(state().focusedMainTab).toBe(chat.id);
    expect(state().activeContent).toBe(a.id);
    const fixed = findTab(state().layout, chat.id)!.tab;
    expect(fixed.scope).toMatchObject({ mode: 'fixed', target: { kind: 'thread', id: 'a' } });
    const read = panelRead(fixed, state().layout, state().activeContent, panelTarget(fixed, state().layout, state().activeContent, null));
    expect(read?.key.owner).toBe(chat.id); expect(read?.threadId).toBe('a');
    await focusOutline(outline);
    expect(state().focusedMainTab).toBe(chat.id);
    act(() => { state().flush(); });
    const saved = loadWorkspace(USER, 'thread').kept;
    expect(saved?.focusedTab).toBe(outline.id); expect(saved?.focusedMainTab).toBe(chat.id); expect(saved?.activeContent).toBe(a.id);
    act(() => { state().reset(); });
    await settled(drawn, requiredHref(a));
    const main = spatialMainGroups(state().layout.main);
    expect(main).toHaveLength(1); expect(main[0]?.tabs.map((tab) => tab.id)).toEqual([a.id, chat.id]);
    expect(main[0]?.active).toBe(chat.id); expect(state().focusedMainTab).toBe(chat.id); expect(state().activeContent).toBe(a.id);
    expect(document.getElementById(`tab-${chat.id}`)).toHaveAttribute('aria-selected', 'true');
    expect(findTab(state().layout, chat.id)?.tab).toBe(fixed); expect(findTab(state().layout, outline.id)?.area).toBe('right');
    expect(panelRead(fixed, state().layout, state().activeContent, panelTarget(fixed, state().layout, state().activeContent, null))).toEqual(read);
    expect(state().unsaved.has(a.id)).toBe(true);
    act(() => { state().focus(a.id); }); await settled(drawn, requiredHref(a));
    expect(bodyToken(a.id)).toBe(token); expect(localInput(a.id)).toHaveValue('Keep the conversation state while Chat is selected');
  });

  it.each(['layout', 'palette'] as const)('%s Close other main tabs keeps the actually moved Chat across groups and preserves sidebar/bottom inspectors', async (surface) => {
    const a = page(address('a')), b = page(address('b', 'version-y'));
    const { chat, outline, inspector } = sidebar([a, b], b.id, true);
    const drawn = draw(requiredHref(b));
    await settled(drawn, requiredHref(b)); await screen.findByTestId(`body-${b.id}`);
    await moveChatFromHeader(chat, 'main');
    expect(state().focusedMainGroup).toBe(findTab(state().layout, a.id)?.group.id);
    expect(state().focusedMainTab).toBe(chat.id); expect(state().activeContent).toBe(b.id);
    const fixed = findTab(state().layout, chat.id)!.tab;
    const target = panelTarget(fixed, state().layout, state().activeContent, null);
    const read = panelRead(fixed, state().layout, state().activeContent, target);
    expect(target).toEqual({ kind: 'thread', id: 'b' });
    expect(read?.key.owner).toBe(chat.id); expect(read?.threadId).toBe('b');
    await focusOutline(outline);
    const right = state().layout.right, bottom = state().layout.bottom;
    act(() => { if (surface === 'layout') runLayout('close-other-main'); else runPalette('view.close-other-main'); });
    await settled(drawn, '/workbench');
    expect(spatialMainGroups(state().layout.main).flatMap((group) => group.tabs.map((tab) => tab.id))).toEqual([chat.id]);
    expect(state().closedTabs.map((entry) => entry.tab.id)).toEqual([a.id, b.id]);
    expect(state().focusedMainTab).toBe(chat.id); expect(state().focusedTab).toBe(outline.id); expect(state().activeContent).toBeNull();
    expect(state().layout.right).toEqual(right); expect(state().layout.bottom).toEqual(bottom);
    expect(findTab(state().layout, inspector.id)?.area).toBe('bottom');
    expect(findTab(state().layout, chat.id)?.tab).toBe(fixed);
    expect(panelTarget(fixed, state().layout, state().activeContent, null)).toEqual(target);
    expect(panelRead(fixed, state().layout, state().activeContent, target)).toEqual(read);
    expect(document.getElementById(`tab-${chat.id}`)).toHaveAttribute('aria-selected', 'true');
  });
});

describe('canonical admission has one exact workspace owner', () => {
  it.each([
    { name: 'home', suffix: '?tab=activity#review-anchor', tail: '' },
    { name: 'settings', suffix: '?setting=usage.limit#review-anchor', tail: '/settings/general' },
  ])('waits for the actual pending $name loader before opening one canonical page with its query/hash', async ({ suffix, tail }) => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const gate = holdCanonical();
    const alias = `/org/${ORG_ID}${tail}${suffix}`, canonical = `/org/${ORG_ADDRESS}${tail}${suffix}`;
    let navigation = Promise.resolve();
    act(() => { navigation = drawn.router.navigate({ href: alias }); });
    await waitFor(() => {
      expect(canonicalCalls).toEqual([ORG_ID]);
      expect(drawn.router.state.status).toBe('pending');
      expect(drawn.router.state.location.href).toBe(alias);
    });
    expect(ids()).toEqual([a.id]); expect(body(a.id)).toBeVisible();
    expect(bodies.filter((entry) => entry.kind === 'org' || entry.kind === 'org-settings')).toEqual([]);
    await act(async () => { gate.resolve(department); await navigation; });
    await settled(drawn, canonical);
    const made = pageTabs().find((tab) => tab.id !== a.id);
    if (made === undefined) throw new Error('Canonical navigation did not create its one new owner.');
    await screen.findByTestId(`body-${made.id}`);
    expect(ids()).toEqual([a.id, made.id]); expect(hrefOf(made.id)).toBe(canonical);
    expect(within(body(made.id)).getByLabelText('Org address')).toHaveTextContent(ORG_ADDRESS);
    expect(bodies.filter((entry) => entry.kind === 'org' || entry.kind === 'org-settings').every((entry) => !entry.pathname.includes(ORG_ID))).toBe(true);
    expect(canonicalCalls).toEqual([ORG_ID]); expect(state().activeContent).toBe(made.id);
  });

  it('canonicalizes an explicitly opened beside alias in that same tab, without an extra canonical instance', async () => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const gate = holdCanonical();
    const alias = `/org/${ORG_ID}?tab=members#people-anchor`;
    const canonical = `/org/${ORG_ADDRESS}?tab=members#people-anchor`;
    const owner = openOwned(alias, 'beside');
    const group = findTab(state().layout, owner)?.group.id;
    await waitFor(() => { expect(canonicalCalls).toEqual([ORG_ID]); });
    expect(ids()).toEqual([a.id, owner]);
    expect(bodies.filter((entry) => entry.kind === 'org')).toEqual([]);
    await act(async () => { gate.resolve(department); await gate.promise; });
    await settled(drawn, canonical);
    await screen.findByTestId(`body-${owner}`);
    expect(ids()).toEqual([a.id, owner]); expect(hrefOf(owner)).toBe(canonical);
    expect(findTab(state().layout, owner)?.group.id).toBe(group); expect(state().activeContent).toBe(owner);
    expect(pageNavigationOrigin()).toBeNull();
  });

  it('a focused duplicate’s owned replace and canonical echo keep its identity and the other duplicate’s source', async () => {
    const a = page(address('same-thread', 'version-x')), b = page(address('same-thread', 'version-y'));
    seed([a, b], b.id, true);
    const drawn = draw(requiredHref(b));
    await Promise.all([screen.findByTestId(`body-${a.id}`), screen.findByTestId(`body-${b.id}`)]); await settled(drawn, requiredHref(b));
    const gate = holdCanonical();
    const alias = `/org/${ORG_ID}?tab=activity#owner-anchor`, canonical = `/org/${ORG_ADDRESS}?tab=activity#owner-anchor`;
    let navigation = Promise.resolve();
    act(() => { navigation = capturedNavigate(b.id, { href: alias, replace: true }); });
    await waitFor(() => { expect(canonicalCalls).toEqual([ORG_ID]); });
    expect(ids()).toEqual([a.id, b.id]); expect(hrefOf(a.id)).toBe(requiredHref(a));
    expect(bodies.filter((entry) => entry.kind === 'org')).toEqual([]);
    await act(async () => { gate.resolve(department); await navigation; });
    await settled(drawn, canonical);
    expect(await screen.findByTestId(`body-${b.id}`)).toHaveAttribute('data-kind', 'org');
    expect(ids()).toEqual([a.id, b.id]); expect(hrefOf(b.id)).toBe(canonical); expect(hrefOf(a.id)).toBe(requiredHref(a));
    expect(body(a.id)).toHaveAttribute('data-continuation', 'version-x');
    expect(state().activeContent).toBe(b.id); expect(state().focusedTab).toBe(b.id); expect(pageNavigationOrigin()).toBeNull();
  });

  it('a retained alias reload is canonicalized in its stored owner instead of allocating another tab', async () => {
    const a = page(address('a'));
    const alias = `/org/${ORG_ID}?tab=activity#restored-anchor`, canonical = `/org/${ORG_ADDRESS}?tab=activity#restored-anchor`;
    const restored = page(alias);
    seed([a, restored], restored.id); state().leave();
    const gate = holdCanonical();
    const drawn = draw(alias);
    await waitFor(() => { expect(canonicalCalls).toEqual([ORG_ID]); });
    expect(bodies.filter((entry) => entry.kind === 'org')).toEqual([]);
    await act(async () => { gate.resolve(department); await gate.promise; });
    await settled(drawn, canonical);
    expect(await screen.findByTestId(`body-${restored.id}`)).toHaveAttribute('data-kind', 'org');
    expect(ids()).toEqual([a.id, restored.id]); expect(hrefOf(restored.id)).toBe(canonical); expect(hrefOf(a.id)).toBe(requiredHref(a));
    expect(state().activeContent).toBe(restored.id);
  });

  it('closing an owned alias while its canonical read is held prevents the late result from reopening or stealing focus', async () => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const gate = holdCanonical();
    const owner = openOwned(`/org/${ORG_ID}?tab=activity#closed-owner`, 'beside');
    await waitFor(() => { expect(canonicalCalls).toEqual([ORG_ID]); });
    act(() => { state().focus(a.id); state().close({ kind: 'tab', tab: owner }); });
    await act(async () => { gate.resolve(department); await gate.promise; });
    await settled(drawn, requiredHref(a));
    expect(ids()).toEqual([a.id]); expect(findTab(state().layout, owner)).toBeNull(); expect(state().activeContent).toBe(a.id);
    expect(state().closedTabs.map((entry) => entry.tab.id)).toEqual([owner]);
    expect(bodies.filter((entry) => entry.kind === 'org')).toEqual([]);
  });
});

describe('addressed failures stay local and keep the attempted destination', () => {
  it.each(['warm', 'cold'] as const)('%s unknown internal navigation owns a NotFound tab and survives unrelated layout writes', async (start) => {
    const a = page(address('a'));
    if (start === 'warm') seed([a], a.id);
    const drawn = draw(start === 'warm' ? requiredHref(a) : '/does-not-exist?source=fixture#missing');
    if (start === 'warm') {
      await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
      await act(async () => { await drawn.router.navigate({ href: '/does-not-exist?source=fixture#missing' }); });
    }
    const missing = '/does-not-exist?source=fixture#missing';
    await settled(drawn, missing);
    const failed = pageTabs().find((tab) => hrefOf(tab.id) === missing);
    if (failed === undefined) throw new Error('No addressed owner retained the missing URL.');
    expect(within(ownedView(failed.id)).getByRole('heading', { name: i18n.t('shell.notFound.title') })).toBeVisible();
    expect(ids()).toEqual(start === 'warm' ? [a.id, failed.id] : [failed.id]);
    expect(bodies.some((entry) => entry.owner === failed.id)).toBe(false);
    act(() => { state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, size: layout.bottom.size + 1 } })); });
    await settled(drawn, missing);
    expect(state().activeContent).toBe(failed.id); expect(hrefOf(failed.id)).toBe(missing);
    expect(within(ownedView(failed.id)).getByRole('heading', { name: i18n.t('shell.notFound.title') })).toBeVisible();
  });

  it.each(['navigate', 'beside'] as const)('invalid admin section via %s is NotFound before any Management body mounts', async (how) => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const destination = '/admin/not-a-section?source=fixture#missing-section';
    if (how === 'navigate') await act(async () => { await drawn.router.navigate({ href: destination }); });
    else act(() => { openPage(destination, 'beside'); });
    await settled(drawn, destination);
    const failed = pageTabs().find((tab) => tab.id !== a.id);
    if (failed === undefined) throw new Error('The invalid destination did not retain a local owner.');
    expect(within(ownedView(failed.id)).getByRole('heading', { name: i18n.t('shell.notFound.title') })).toBeVisible();
    expect(ids()).toEqual([a.id, failed.id]); expect(hrefOf(a.id)).toBe(requiredHref(a));
    expect(bodies.filter((entry) => entry.kind === 'management')).toEqual([]);
  });

  it('the exact canonicalizer refusal reaches its owner, with no alias body or unrelated pane replacement', async () => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const token = bodyToken(a.id), gate = holdCanonical();
    const destination = `/org/${ORG_ID}?tab=activity#denied-anchor`;
    const refusal = new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Not allowed', detail: 'Synthetic org view refusal', status: 403, request_id: 'synthetic-canonical-refusal' }, null);
    let navigation = Promise.resolve();
    act(() => { navigation = drawn.router.navigate({ href: destination }); });
    await waitFor(() => { expect(canonicalCalls).toEqual([ORG_ID]); });
    await act(async () => { gate.reject(refusal); await navigation; });
    await settled(drawn, destination);
    const failed = pageTabs().find((tab) => tab.id !== a.id);
    if (failed === undefined) throw new Error('The refused destination lost its owner.');
    await waitFor(() => { expect(within(ownedView(failed.id)).getByRole('alert')).toBeVisible(); });
    expect(usePageFailures.getState().byTab.get(failed.id)).toBe(refusal);
    expect(ids()).toEqual([a.id, failed.id]); expect(hrefOf(failed.id)).toBe(destination); expect(hrefOf(a.id)).toBe(requiredHref(a));
    expect(bodies.filter((entry) => entry.kind === 'org')).toEqual([]);
    act(() => { state().focus(a.id); }); await settled(drawn, requiredHref(a));
    expect(bodyToken(a.id)).toBe(token);
  });

  it('Retry reruns the failed owner and canonicalizes that same tab, preserving the neighboring body and its state', async () => {
    const a = page(address('a'));
    seed([a], a.id);
    const drawn = draw(requiredHref(a));
    await screen.findByTestId(`body-${a.id}`); await settled(drawn, requiredHref(a));
    const token = bodyToken(a.id);
    fireEvent.change(localInput(a.id), { target: { value: 'Keep the unrelated owner intact' } });
    const first = holdCanonical();
    const alias = `/org/${ORG_ID}?tab=activity#retry-anchor`, canonical = `/org/${ORG_ADDRESS}?tab=activity#retry-anchor`;
    let navigation = Promise.resolve();
    act(() => { navigation = drawn.router.navigate({ href: alias }); });
    await waitFor(() => { expect(canonicalCalls).toEqual([ORG_ID]); });
    await act(async () => { first.reject(new Error('Synthetic canonical transport failure')); await navigation; });
    await settled(drawn, alias);
    const failed = pageTabs().find((tab) => tab.id !== a.id);
    if (failed === undefined) throw new Error('The failed destination did not retain its identity.');
    const retry = await within(ownedView(failed.id)).findByRole('button', { name: i18n.t('common.tryAgain') });
    const again = holdCanonical();
    const beforeRetry = canonicalCalls.length;
    fireEvent.click(retry);
    await waitFor(() => { expect(canonicalCalls).toHaveLength(beforeRetry + 1); });
    expect(canonicalCalls.every((id) => id === ORG_ID)).toBe(true);
    await act(async () => { again.resolve(department); await again.promise; });
    await settled(drawn, canonical);
    expect(await screen.findByTestId(`body-${failed.id}`)).toHaveAttribute('data-kind', 'org');
    expect(ids()).toEqual([a.id, failed.id]); expect(hrefOf(failed.id)).toBe(canonical); expect(usePageFailures.getState().byTab.has(failed.id)).toBe(false);
    act(() => { state().focus(a.id); }); await settled(drawn, requiredHref(a));
    expect(bodyToken(a.id)).toBe(token); expect(localInput(a.id)).toHaveValue('Keep the unrelated owner intact');
  });
});

it('settling a cancelled alias lookup synchronizes the later focused pane even without another metadata update', async () => {
  const gate = holdCanonical();
  const a = page(address('earlier')), b = page(`/org/${ORG_ID}`), c = page(address('later'));
  seed([a, b, c], a.id, true); const drawn = draw(requiredHref(a)); await settled(drawn, requiredHref(a));
  await waitFor(() => expect(canonicalCalls).toEqual([ORG_ID]));
  const before = drawn.router.state.location.state.__TSR_key;
  await act(async () => {
    markPageNavigation(b.id, requiredHref(b));
    drawn.history.push(requiredHref(a));
    await Promise.resolve();
  });
  await waitFor(() => expect(drawn.router.state.location.state.__TSR_key).not.toBe(before));
  act(() => { state().focus(c.id); markPageNavigation(c.id, requiredHref(c)); });
  expect(state().activeContent).toBe(c.id);
  await act(async () => { gate.resolve(department); await gate.promise; });
  await settled(drawn, requiredHref(c));
  expect(state().activeContent).toBe(c.id); expect(ids()).toEqual([a.id, b.id, c.id]);
  expect(hrefOf(b.id)).toBe(`/org/${ORG_ADDRESS}`);
});

it('an earlier focused URL acknowledgement cannot leave the address bar behind a later tab selection', async () => {
  const a = page(address('kept-attention')), b = page(address('earlier-echo'));
  seed([a, b], a.id); const drawn = draw(requiredHref(a)); await settled(drawn, requiredHref(a));
  const stop = drawn.router.subscribe('onResolved', () => {
    if (drawn.router.state.location.href === requiredHref(b)) state().focus(a.id);
  });
  try {
    let navigation: Promise<void> | undefined;
    act(() => {
      state().focus(b.id); markPageNavigation(b.id, requiredHref(b));
      navigation = drawn.router.navigate({ href: requiredHref(b), replace: true });
    });
    await act(async () => { await navigation; });
    await settled(drawn, requiredHref(a));
    expect(state().activeContent).toBe(a.id); expect(ids()).toEqual([a.id, b.id]);
    expect(hrefOf(b.id)).toBe(requiredHref(b));
  } finally { stop(); }
});
