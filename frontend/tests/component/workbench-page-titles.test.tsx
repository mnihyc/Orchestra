/** Restored frame identity stays readable without mounting inactive page readers. */
import { act, cleanup, render, screen, waitFor, type RenderResult } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { AppWindow } from 'lucide-react';
import { useEffect } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { queryClient } from '@/api/query';
import { client } from '@/api/client';
import type * as Live from '@/realtime/live';
import { ViewHost, ViewSlot } from '@/features/workbench/Board';
import { CachedTabNames } from '@/features/workbench/TabNames';
import { allTabs, findTab, makeGroup, makeTab, type Tab } from '@/features/workbench/layout';
import { loadWorkspace } from '@/features/workbench/persistence';
import { pageAddress, parsePageAddress } from '@/features/workbench/PageView';
import { staticPageName } from '@/features/workbench/pageNames';
import { useWorkbench } from '@/features/workbench/store';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { registerView, type ViewProps } from '@/features/workbench/views';
import { FrameReadiness } from '@/shell/FrameReadiness';
import { clearScreenHeading, focusScreenHeading, forgetScreenHeading, useScreenHeading } from '@/shell/screen';
import { useTreeStore, treeWriter } from '@/realtime/stores/tree';
import { currentLocale, i18n, setLocale } from '@/i18n';
import { TooltipProvider } from '@/ui/Tooltip';

const calls = vi.hoisted(() => ({ thread: vi.fn(), document: vi.fn(), fetch: vi.fn<typeof fetch>() }));
vi.mock('@/realtime/live', async (load) => ({ ...await load<typeof Live>(), useThreadFeed: calls.thread, useDocumentFeed: calls.document }));
const state = () => useWorkbench.getState();
const USER = 'static-title-reader', OTHER = 'another-title-reader';
const resources = [
  ['providers', 'Providers', '提供方'], ['agents', 'Agents', '智能体'], ['machines', 'Machines', '机器'],
  ['environments', 'Environments', '环境'], ['templates', 'Templates', '模板'], ['hosts', 'Hosts', '主机'],
  ['mcp', 'MCP servers', 'MCP 服务器'], ['mounts', 'Mounts', '挂载'],
] as const;
const ready = new Set<string>(), mounted: string[] = [], views: RenderResult[] = [];
const owners = new Set<string>();
const savedSession = client.session.getState();
let locale = currentLocale(), unregister: (() => void) | undefined;
function DelayedBody({ tab }: ViewProps) {
  const kind = tab.of?.href?.split(/[?#]/)[0]?.split('/').at(-1);
  const resource = resources.find((entry) => entry[0] === kind);
  useScreenHeading({ title: resource === undefined ? 'Synthetic loaded title' : i18n.t(`screens.${resource[0]}.title`), icon: AppWindow }, ready.has(tab.id));
  useEffect(() => { mounted.push(tab.id); }, [tab.id]);
  return <div data-testid={`pending-body-${tab.id}`} />;
}
function Frame() {
  const layout = useWorkbench((value) => value.layout), active = useWorkbench((value) => value.focusedMainTab), named = useWorkbench((value) => value.named);
  const tab = active === null ? null : findTab(layout, active)?.tab;
  return <FrameReadiness.Provider value={{ ready: true, pending: null }}><ViewHost layout={layout} />
    {tab !== null && tab !== undefined && <ViewSlot tab={tab} />}
    {allTabs(layout).filter((one) => one.view === 'page').map((one) => <output key={one.id} data-testid={`title-${one.id}`}>{named.get(one.id)?.title ?? i18n.t('views.screen.title')}</output>)}
  </FrameReadiness.Provider>;
}
function drawFrame(): RenderResult {
  const view = render(<QueryClientProvider client={queryClient}><TooltipProvider><Frame /></TooltipProvider></QueryClientProvider>);
  views.push(view); return view;
}
function tabsForResources(): Tab[] {
  return resources.map(([kind]) => { const tab = makeTab('page', { href: `/resources/${kind}?q=unchanged&status=enabled#filter` }); owners.add(tab.id); return tab; });
}
function seed(tabs: Tab[], selected = tabs[0]!) {
  state().enter(USER, 'page');
  state().change((layout) => ({ ...layout, main: makeGroup(tabs, selected.id),
    left: { ...layout.left, open: false, active: null, containers: [] }, right: { ...layout.right, open: false, active: null, containers: [] },
    bottom: { ...layout.bottom, open: false, group: makeGroup() },
  }));
  state().focus(selected.id);
}
function freshDocument() {
  for (const view of views.splice(0)) view.unmount();
  state().leave(); clearViewOpenings(); focusScreenHeading(null); clearScreenHeading();
  for (const id of owners) forgetScreenHeading(id);
  state().enter(USER, 'page');
}
beforeEach(async () => {
  locale = currentLocale(); await setLocale('en'); state().leave(); localStorage.clear(); queryClient.clear(); treeWriter.clear();
  clearViewOpenings(); focusScreenHeading(null); clearScreenHeading(); ready.clear(); mounted.length = 0;
  client.session.setState({ status: 'signed_in', userId: USER });
  calls.thread.mockClear(); calls.document.mockClear(); calls.fetch.mockReset().mockRejectedValue(new Error('Inactive identity must not fetch'));
  vi.stubGlobal('fetch', calls.fetch);
  unregister = registerView({ name: 'page', title: 'views.screen.title', about: 'views.screen.about', icon: AppWindow, screens: 'all', area: 'main', role: 'content', body: DelayedBody });
});
afterEach(async () => {
  for (const view of views.splice(0)) view.unmount(); cleanup(); unregister?.(); unregister = undefined;
  state().leave(); localStorage.clear(); await queryClient.cancelQueries(); queryClient.clear(); treeWriter.clear();
  clearViewOpenings(); focusScreenHeading(null); clearScreenHeading(); for (const id of owners) forgetScreenHeading(id); owners.clear();
  client.session.setState(savedSession); vi.unstubAllGlobals(); await setLocale(locale);
});

it.each(['en', 'zh-CN'] as const)('keeps the previously active cached title through metadata admission, another focus and two hard reloads in %s', async (language) => {
  await setLocale(language); const tabs = tabsForResources(), previous = tabs[0]!, next = tabs[6]!;
  seed(tabs, previous);
  for (let index = 0; index < tabs.length; index += 1) state().setNamed(tabs[index]!.id, { title: resources[index]![language === 'en' ? 1 : 2], source: 'view' });
  state().flush(); freshDocument();
  const ids = allTabs(state().layout).filter((tab) => tab.view === 'page').map((tab) => tab.id);
  const hrefs = tabs.map(pageAddress); drawFrame();
  await screen.findByTestId(`pending-body-${previous.id}`);
  await waitFor(() => expect(state().named.get(previous.id)?.title).toBe(resources[0][language === 'en' ? 1 : 2]));
  expect(state().named.get(previous.id)?.source).toBe('cache');
  act(() => state().focus(next.id)); await screen.findByTestId(`pending-body-${next.id}`);
  // The previous page never supplied its new heading before Activity paused it.
  expect(ready.has(previous.id)).toBe(false);
  for (let index = 0; index < tabs.length; index += 1) expect(state().named.get(tabs[index]!.id)?.title).toBe(resources[index]![language === 'en' ? 1 : 2]);
  expect(new Set(mounted)).toEqual(new Set([previous.id, next.id]));
  await act(() => window.dispatchEvent(new Event('pagehide')));
  const saved = loadWorkspace(USER, 'page').kept;
  expect(saved?.titles?.[previous.id]?.title).toBe(resources[0][language === 'en' ? 1 : 2]);
  freshDocument(); mounted.length = 0; drawFrame(); await screen.findByTestId(`pending-body-${next.id}`);
  expect(new Set(mounted)).toEqual(new Set([next.id]));
  expect(allTabs(state().layout).filter((tab) => tab.view === 'page').map((tab) => tab.id)).toEqual(ids);
  expect(tabs.map((tab) => pageAddress(findTab(state().layout, tab.id)!.tab))).toEqual(hrefs);
  for (let index = 0; index < tabs.length; index += 1) expect(screen.getByTestId(`title-${tabs[index]!.id}`)).toHaveTextContent(resources[index]![language === 'en' ? 1 : 2]);
  expect(calls.fetch).not.toHaveBeenCalled(); expect(calls.thread).not.toHaveBeenCalled(); expect(calls.document).not.toHaveBeenCalled();
});

it.each(['en', 'zh-CN'] as const)('repairs a restored workspace without any title snapshots in %s without mounting a single page', async (language) => {
  await setLocale(language); const tabs = tabsForResources(); seed(tabs);
  const layout = state().layout, hrefs = tabs.map(pageAddress);
  state().flush(); freshDocument();
  render(<CachedTabNames layout={state().layout} />);
  await waitFor(() => expect(state().named.size).toBe(tabs.length));
  for (let index = 0; index < tabs.length; index += 1) {
    const name = state().named.get(tabs[index]!.id);
    expect(name?.title).toBe(resources[index]![language === 'en' ? 1 : 2]); expect(name?.icon).toBeDefined(); expect(name?.source).toBe('cache');
  }
  expect(state().layout).toEqual(layout); expect(tabs.map((tab) => pageAddress(findTab(state().layout, tab.id)!.tab))).toEqual(hrefs);
  expect(mounted).toEqual([]); expect(calls.fetch).not.toHaveBeenCalled(); expect(calls.thread).not.toHaveBeenCalled(); expect(calls.document).not.toHaveBeenCalled();
});

it('retranslates cached public names without borrowing a different pane or acquiring metadata readers', async () => {
  await setLocale('en'); const tabs = tabsForResources(); seed(tabs); render(<CachedTabNames layout={state().layout} />);
  await waitFor(() => expect(state().named.get(tabs[0]!.id)?.title).toBe('Providers'));
  await act(async () => { await setLocale('zh-CN'); });
  await waitFor(() => expect(state().named.get(tabs[0]!.id)?.title).toBe('提供方'));
  expect(state().activeContent).toBe(tabs[0]!.id); expect(mounted).toEqual([]); expect(calls.fetch).not.toHaveBeenCalled();
});

it('lets the real page heading take ownership after admission even when its text equals the static fallback', async () => {
  const tabs = tabsForResources(); seed(tabs); const view = drawFrame();
  await screen.findByTestId(`pending-body-${tabs[0]!.id}`);
  await waitFor(() => expect(state().named.get(tabs[0]!.id)?.source).toBe('cache'));
  ready.add(tabs[0]!.id); view.rerender(<QueryClientProvider client={queryClient}><TooltipProvider><Frame /></TooltipProvider></QueryClientProvider>);
  await waitFor(() => expect(state().named.get(tabs[0]!.id)?.source).toBe('view'));
  expect(state().named.get(tabs[0]!.id)?.title).toBe('Providers');
  expect(calls.fetch).not.toHaveBeenCalled();
});

it('preserves a current view/refusal name instead of claiming a successful static or cached resource title', async () => {
  const tabs = tabsForResources(); seed(tabs);
  state().setNamed(tabs[0]!.id, { title: 'Current access refusal', source: 'view' });
  render(<CachedTabNames layout={state().layout} />);
  await waitFor(() => expect(state().named.has(tabs[1]!.id)).toBe(true));
  expect(state().named.get(tabs[0]!.id)).toEqual({ title: 'Current access refusal', source: 'view' });
  expect(calls.fetch).not.toHaveBeenCalled();
});

it('keeps a resource detail’s already authorized name and path without guessing names from IDs or opening its reader', () => {
  const detail = makeTab('page', { href: '/resources/providers/opaque-private-id' }); owners.add(detail.id); seed([detail]);
  const cached = { title: 'Already authorized label', path: 'Redacted place / …', source: 'cache' as const };
  state().setNamed(detail.id, cached); render(<CachedTabNames layout={state().layout} />);
  expect(state().named.get(detail.id)).toEqual(cached); expect(calls.fetch).not.toHaveBeenCalled();
  const target = parsePageAddress(detail.of!.href!); expect(target).not.toBeNull(); expect(staticPageName(target!, i18n.t)).toBeNull();
});

it('does not carry another person’s title snapshots into the next workspace', () => {
  const tabs = tabsForResources(); seed(tabs); state().setNamed(tabs[0]!.id, { title: 'Person-specific remembered name', source: 'cache' }); state().flush();
  act(() => state().enter(OTHER, 'page'));
  expect(state().named.size).toBe(0); expect(allTabs(state().layout).some((tab) => tab.id === tabs[0]!.id)).toBe(false);
  render(<CachedTabNames layout={state().layout} />);
  expect([...state().named.values()].some((name) => name.title === 'Person-specific remembered name')).toBe(false);
  expect(calls.fetch).not.toHaveBeenCalled();
});

it.each([
  ['/resources', 'Resources'], ['/everything', 'Everything'], ['/search?q=kept', 'Search results'], ['/inbox', 'Inbox'],
  ['/approvals', 'Approvals'], ['/shared', 'Shared with me'], ['/help?part=keys', 'Help'],
  ['/settings/profile', 'Profile'], ['/settings/security', 'Security'], ['/settings/notifications', 'Notifications'],
  ['/settings/tokens', 'API tokens'], ['/settings/exports', 'Your exports'], ['/settings/interface', 'Interface'],
  ['/usage?tab=performance', 'Usage'], ['/settings/your-page', 'Your page'],
  ['/settings', 'Profile'], ['/settings/usage', 'Usage'], ['/admin/users', 'Users'],
] as const)('names the same non-object page class at %s from public metadata alone', async (href, title) => {
  await setLocale('en'); const target = parsePageAddress(href);
  expect(target).not.toBeNull(); expect(staticPageName(target!, i18n.t)?.title).toBe(title);
  expect(calls.fetch).not.toHaveBeenCalled(); expect(calls.thread).not.toHaveBeenCalled(); expect(calls.document).not.toHaveBeenCalled();
});

it('does not infer a native/admin authority or resource name from a merely similar or invalid route', () => {
  for (const href of ['/resources/providers/one', '/resources/agents/one', '/document-folder/one', '/doc/one', '/chat/one', '/help/one', '/inspect/document/one', '/u/person', '/admin/unknown']) {
    const target = parsePageAddress(href); expect(target).not.toBeNull(); expect(staticPageName(target!, i18n.t)).toBeNull();
  }
  expect(useTreeStore.getState().byOrg.size).toBe(0); expect(calls.fetch).not.toHaveBeenCalled();
});
