/** Real router/store/Board/contexts; only native read replies and heavy feature bodies are synthetic. */
import { act, cleanup, fireEvent, render, screen, waitFor, type RenderResult } from '@testing-library/react';
import { useLayoutEffect } from 'react';
import { QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider, type AnyRoute } from '@tanstack/react-router';
import { File, PanelsTopLeft } from 'lucide-react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Workspace from '@/api/workspace';
import type * as Nodes from '@/api/nodes';
import { ApiError, client } from '@/api/client';
import { queryClient } from '@/api/query';
import { hydrateThreadOpening, threadKeys } from '@/api/threads';
import { selectThreadContinuation } from '@/api/threadRead';
import { FileTab, openFile } from '@/features/files/FileTabs';
import type * as FileContent from '@/features/files/content';
import type { FileHead } from '@/features/files/content';
import type { ViewerProps } from '@/features/files/viewers';
import { FilesView } from '@/features/inspector/Files';
import type * as DirectoryTreeModule from '@/features/files/useDirectoryTree';
import { contentAddress, contentHref, contentLocation, type ContentLocation } from '@/features/workbench/contentAddress';
import { openContent } from '@/features/workbench/contentNavigation';
import { allTabs, closeTabs, findTab, makeGroup, makeTab, newId, setPinned, setTabTarget, type Tab } from '@/features/workbench/layout';
import { clearPageNavigation, pageNavigationOrigin, usePageFailures } from '@/features/workbench/pageNavigation';
import { PageView } from '@/features/workbench/PageView';
import { fixPanelScope, moveScoped, setPanelScope } from '@/features/workbench/panelScope';
import { registerView } from '@/features/workbench/views';
import { useWorkbench } from '@/features/workbench/store';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { threadWriter, threadViewOf } from '@/realtime/stores/thread';
import { treeWriter } from '@/realtime/stores/tree';
import { contentThreadRead, resultThreadRead } from '@/realtime/threadScope';
import { workspaceThreadRead } from '@/features/workbench/workspaceRead';
import { useThreadWindowLifetime } from '@/features/workbench/threadWindows';
import '@/features/search/ResultView';
// Admit the real lazy module before timing reader changes, including its first use in this file.
import { ResultThread } from '@/features/search/ResultContent';
import { clearScreenHeading, focusScreenHeading } from '@/shell/screen';
import { TooltipProvider } from '@/ui/Tooltip';
import { Route as ThreadRoute } from '@/routes/thread/$threadId/index';
import { Route as DocRoute } from '@/routes/doc/$documentId/index';
import { Route as ChatRoute } from '@/routes/chat/$chatId/index';
import { Route as PlaceChatRoute } from '@/routes/place-chat/$nodeId/index';
import { RoutingFrame } from './workbench-routing/frame';
import { delayedReads, feedCalls, FileWitness, heldFeeds, InspectorWitness, nativeRows, nativeNodes, nativeThread, useNativeFeed, USER } from './workbench-content/fixture';
import { World } from '../doubles/world';
import { ACME } from '../doubles/fixtures';
import { threadOpening } from '../doubles/thread-opening';

vi.mock('@/realtime/live', async (original) => {
  const actual = await original<typeof Live>();
  const { useNativeFeed } = await import('./workbench-content/fixture');
  return { ...actual, useThreadFeed: useNativeFeed, useDocumentFeed: () => undefined };
});
vi.mock('@/features/thread/Thread', async () => {
  const { FullThread } = await import('./workbench-content/fixture'); return { ThreadScreen: FullThread };
});
vi.mock('@/features/documents/Document', async () => {
  const { FullDocument } = await import('./workbench-content/fixture'); return { DocumentPage: FullDocument };
});
vi.mock('@/features/chat/ChatView', async () => {
  const { FullChat } = await import('./workbench-content/fixture'); return { ChatView: FullChat, chatTitle: () => 'Actual chat', chatPageTitle: () => 'Actual chat' };
});
vi.mock('@/features/timeline/Timeline', () => ({ Timeline: () => <div role="log" /> }));
vi.mock('@/api/workspace', async (original) => {
  const actual = await original<typeof Workspace>();
  const { nativeEnvironment: environment } = await import('./workbench-content/fixture');
  return { ...actual, environmentQuery: (id: string) => ({ ...actual.environmentQuery(id), queryFn: () => Promise.resolve(environment(id)) }) };
});
vi.mock('@/api/nodes', async (original) => {
  const actual = await original<typeof Nodes>();
  const { nativeNodes: nodes } = await import('./workbench-content/fixture');
  return { ...actual, nodeQuery: (id: string) => ({ ...actual.nodeQuery(id), queryFn: () => {
    const row = nodes.get(id);
    return row === undefined ? Promise.reject(new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: '', detail: 'Synthetic native node refusal', status: 404, request_id: 'owned-node' }, null)) : Promise.resolve(row);
  } }) };
});

// This witness receives the real FileTabs props; actual CodeView dispatch is covered separately.
vi.mock('@/features/files/lazy', () => ({ lazyPart: () => function FileViewerWitness({ line, lineRequest }: ViewerProps) {
  return <section><output aria-label="Addressed file line" data-line={line ?? 'none'} data-request={lineRequest ?? 'none'} />
    <textarea aria-label="Retained viewer draft" defaultValue="Original file bytes" /></section>;
} }));
vi.mock('@/features/files/content', async (original) => {
  const actual = await original<typeof FileContent>();
  const head: FileHead = { stat: { path: 'main.rs', name: 'main.rs', type: 'file', size: 19, mode: 420,
    cached: true, mtime: '2026-10-17T00:00:00Z', link_target: null, media_type: 'text/plain', is_text: true, hash: 'synthetic-file-bytes' },
    brought: { state: 'ready', hash: 'synthetic-file-bytes', size: 19 } };
  return { ...actual, useFileHead: () => ({ head, error: null, loading: false, reload: () => Promise.resolve() }) };
});

vi.mock('@/features/files/useDirectoryTree', async (original) => {
  const actual = await original<typeof DirectoryTreeModule>();
  const { useState } = await import('react');
  const directories: ReturnType<typeof actual.useDirectoryTree>['directories'] = new Map();
  const expanded: ReturnType<typeof actual.useDirectoryTree>['expanded'] = new Set();
  return { ...actual, useDirectoryTree: (): ReturnType<typeof actual.useDirectoryTree> => {
    const [tree] = useState(() => new actual.DirectoryTree());
    return { identity: 'synthetic-native-empty-directory', directories, expanded, capacity: null, tree };
  } };
});

const state = () => useWorkbench.getState();
const savedSession = client.session.getState(), savedOptions = queryClient.getDefaultOptions();
const unregister: (() => void)[] = [], mounted: RenderResult[] = [], histories: ReturnType<typeof createMemoryHistory>[] = [];
beforeEach(() => {
  state().leave(); localStorage.clear(); threadWriter.clear(); queryClient.clear(); treeWriter.clear();
  nativeRows.clear(); nativeNodes.clear(); heldFeeds.clear(); delayedReads.clear(); feedCalls.length = 0;
  clearPageNavigation(pageNavigationOrigin()); usePageFailures.setState({ byTab: new Map() }); clearViewOpenings();
  focusScreenHeading(null); clearScreenHeading();
  client.session.setState({ status: 'signed_in', userId: USER });
  queryClient.setDefaultOptions({ ...savedOptions, queries: { ...savedOptions.queries, retry: false } });
  for (const name of ['screen', 'page'] as const) unregister.push(registerView({ name, title: 'views.screen.title', about: 'views.screen.about', icon: PanelsTopLeft, screens: 'all', area: 'main', role: 'content', body: PageView }));
  for (const name of ['file', 'difference'] as const) unregister.push(registerView({ name, title: 'files.fileView.title', about: 'files.fileView.about', icon: File, screens: 'all', area: 'main', role: 'content', body: FileWitness }));
  for (const name of ['outline', 'given', 'diagnostics', 'files'] as const) unregister.push(registerView({ name, title: 'views.outline.title', about: 'views.outline.about', icon: PanelsTopLeft, screens: 'all', area: 'right', body: InspectorWitness }));
  for (const name of ['navigation', 'resource-navigation'] as const) unregister.push(registerView({ name, title: 'views.navigation.title', about: 'views.navigation.about', icon: PanelsTopLeft, screens: 'all', area: 'left', body: () => null }));
});
afterEach(async () => {
  act(() => { client.session.setState({ status: 'signed_out', userId: null }); state().leave(); });
  for (const view of mounted.splice(0)) view.unmount(); cleanup();
  for (const history of histories.splice(0)) history.destroy();
  await queryClient.cancelQueries(); queryClient.clear(); threadWriter.clear(); treeWriter.clear();
  queryClient.setDefaultOptions(savedOptions); for (const remove of unregister.splice(0)) remove();
  clearPageNavigation(pageNavigationOrigin()); usePageFailures.setState({ byTab: new Map() }); clearViewOpenings();
  focusScreenHeading(null); clearScreenHeading(); client.session.setState(savedSession); localStorage.clear();
});
function seed(tabs: Tab[], selected: string, panels: Tab[] = []) {
  state().enter(USER, 'thread');
  const groups = tabs.map((tab) => makeGroup([tab]));
  state().change((layout) => ({ ...layout,
    main: groups.length === 1 ? groups[0]! : { kind: 'split', id: newId('s'), direction: 'row', children: groups, sizes: groups.map(() => 100 / groups.length) },
    left: { ...layout.left, open: false, containers: [], active: null },
    right: { ...layout.right, open: panels.length > 0, active: 'details', containers: panels.length === 0 ? [] : [{ id: 'details-group', name: 'details', active: panels[0]!.id, sections: panels.map((tab) => ({ tab, open: true, hidden: false, size: 1 })) }] },
    bottom: { ...layout.bottom, open: false, group: makeGroup() },
  }));
  state().focus(selected); state().flush();
}
function draw(href: string) {
  const root = createRootRoute({ component: Outlet, shellComponent: RoutingFrame });
  const bind = (route: AnyRoute, path: string) => { Object.assign(route.options, { id: path, path, getParentRoute: () => root }); };
  bind(ThreadRoute, '/thread/$threadId/'); bind(DocRoute, '/doc/$documentId/'); bind(ChatRoute, '/chat/$chatId/'); bind(PlaceChatRoute, '/place-chat/$nodeId/');
  const neutral = createRoute({ getParentRoute: () => root, path: '/workbench', staticData: { screen: 'page' }, component: () => null });
  const history = createMemoryHistory({ initialEntries: [href] }); histories.push(history);
  const router = createRouter({ routeTree: root.addChildren([ThreadRoute, DocRoute, ChatRoute, PlaceChatRoute, neutral]), history, defaultPendingMs: 0, defaultPendingMinMs: 0 });
  const view = render(<QueryClientProvider client={queryClient}><TooltipProvider><RouterProvider router={router} /></TooltipProvider></QueryClientProvider>);
  mounted.push(view); return { router, history, view };
}
async function settled(drawn: ReturnType<typeof draw>, href?: string) {
  await waitFor(() => { expect(drawn.router.state.status).toBe('idle'); expect(drawn.router.state.isLoading).toBe(false); if (href !== undefined) expect(drawn.router.state.location.href).toBe(href); });
}
function page(id: string, continuation?: string): Tab { return makeTab('page', { href: `/thread/${id}${continuation === undefined ? '' : `?continuation=${continuation}`}` }); }
function fileLocation(env: string, thread: string | null, path = 'main.rs', line?: number): ContentLocation {
  return { view: 'file', environmentId: env, threadId: thread, path, ...(line === undefined ? {} : { line }) };
}

it('pushes ordinary files, replaces focus echoes, and back/forward recovers exact file/diff targets without losing peers', async () => {
  const a = page('thread-a'); nativeRows.set('thread-a', nativeThread('thread-a')); seed([a], a.id);
  const drawn = draw(a.of!.href!); await settled(drawn, a.of!.href);
  const first = fileLocation('environment-thread-a', 'thread-a', 'one.rs', 31);
  act(() => openFile({ environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'one.rs', line: 31 }));
  await settled(drawn, contentHref(first)); const file = state().activeContent!;
  const diff: ContentLocation = { view: 'difference', environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'main.rs', kind: 'git', item: '', staged: true };
  act(() => { openContent(diff, 'normal', 'push'); }); await settled(drawn, contentHref(diff));
  act(() => drawn.history.back()); await settled(drawn, contentHref(first));
  expect(state().activeContent).toBe(file); expect(contentLocation(findTab(state().layout, file)!.tab)).toEqual(first);
  act(() => drawn.history.forward()); await settled(drawn, contentHref(diff));
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file')).toHaveLength(1);
  expect(allTabs(state().layout).filter((tab) => tab.view === 'difference')).toHaveLength(1);
  expect(findTab(state().layout, a.id)).not.toBeNull();
});

it('an explicit resource action pushes even when reusing its existing tab, preserving the previously focused resource for Back/Forward', async () => {
  const a = page('thread-a'); nativeRows.set('thread-a', nativeThread('thread-a')); seed([a], a.id);
  const drawn = draw(a.of!.href!); await settled(drawn, a.of!.href);
  const first = fileLocation('environment-thread-a', 'thread-a', 'retained.rs', 7);
  act(() => openFile({ environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'retained.rs', line: 7 }));
  await settled(drawn, contentHref(first)); const reused = state().activeContent!;
  const other: ContentLocation = { view: 'difference', environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'other.rs', kind: 'git', item: '', staged: false };
  act(() => { openContent(other, 'normal', 'push'); }); await settled(drawn, contentHref(other)); const prior = state().activeContent!;
  const requested = fileLocation('environment-thread-a', 'thread-a', 'retained.rs', 29);
  act(() => openFile({ environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'retained.rs', line: 29 }));
  await settled(drawn, contentHref(requested));
  expect(state().activeContent).toBe(reused);
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file')).toHaveLength(1);
  act(() => drawn.history.back()); await settled(drawn, contentHref(other));
  expect(state().activeContent).toBe(prior);
  act(() => drawn.history.forward()); await settled(drawn, contentHref(requested));
  expect(state().activeContent).toBe(reused);
  expect(contentLocation(findTab(state().layout, reused)!.tab)).toEqual(requested);
  expect(findTab(state().layout, a.id)).not.toBeNull();
});

it('the real file tab honors Back/Forward and absent addressed lines over an old local jump while repeated requests preserve the viewer', async () => {
  const a = page('thread-a'); nativeRows.set('thread-a', nativeThread('thread-a')); seed([a], a.id);
  unregister.push(registerView({ name: 'file', title: 'files.fileView.title', about: 'files.fileView.about', icon: File, screens: 'all', area: 'main', role: 'content', body: FileTab }));
  const drawn = draw(a.of!.href!); await settled(drawn, a.of!.href);
  const first = fileLocation('environment-thread-a', 'thread-a', 'main.rs', 7);
  act(() => openFile({ environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'main.rs', line: 7 }));
  await settled(drawn, contentHref(first));
  const output = await screen.findByLabelText('Addressed file line'), editor = screen.getByRole('textbox', { name: 'Retained viewer draft' });
  expect(output).toHaveAttribute('data-line', '7');
  fireEvent.change(editor, { target: { value: 'Unsaved local file words' } });
  const addressed = fileLocation('environment-thread-a', 'thread-a', 'main.rs', 42);
  act(() => { openContent(addressed, 'normal', 'push'); }); await settled(drawn, contentHref(addressed));
  expect(output).toHaveAttribute('data-line', '42'); expect(output).toHaveAttribute('data-request', '0');
  act(() => drawn.history.back()); await settled(drawn, contentHref(first));
  expect(output).toHaveAttribute('data-line', '7');
  act(() => drawn.history.forward()); await settled(drawn, contentHref(addressed));
  expect(output).toHaveAttribute('data-line', '42');
  const unanchored = fileLocation('environment-thread-a', 'thread-a', 'main.rs');
  act(() => { openContent(unanchored, 'normal', 'push'); }); await settled(drawn, contentHref(unanchored));
  expect(output).toHaveAttribute('data-line', 'none'); expect(output).toHaveAttribute('data-request', 'none');
  act(() => openFile({ environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'main.rs', line: 42 }));
  await settled(drawn, contentHref(addressed)); const previous = Number(output.getAttribute('data-request'));
  act(() => openFile({ environmentId: 'environment-thread-a', threadId: 'thread-a', path: 'main.rs', line: 42 }));
  expect(Number(output.getAttribute('data-request'))).toBeGreaterThan(previous);
  expect(screen.getByRole('textbox', { name: 'Retained viewer draft' })).toBe(editor);
  expect(editor).toHaveValue('Unsaved local file words'); expect(output).toHaveAttribute('data-line', '42');
});

it('cold file-only URL and saved file-only reload keep a real resource; the final close stays neutral', async () => {
  nativeRows.set('thread-a', nativeThread('thread-a'));
  const location = fileLocation('environment-thread-a', 'thread-a', 'only.rs', 9), drawn = draw(contentHref(location));
  await settled(drawn, contentHref(location));
  const id = state().activeContent!; expect(findTab(state().layout, id)?.tab.view).toBe('file');
  expect(allTabs(state().layout).some((tab) => tab.view === 'page')).toBe(false);
  state().flush(); drawn.view.unmount();
  act(() => { state().leave(); state().enter(USER, 'page'); });
  const restored = draw(contentHref(location)); await settled(restored, contentHref(location));
  expect(state().activeContent).toBe(id);
  act(() => state().close({ kind: 'main' })); await settled(restored, '/workbench');
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file' || tab.view === 'page')).toEqual([]);
});

it('a fixed Files A panel never reinterprets the addressed B old file link, including numeric names and same-door line changes', async () => {
  const a = page('thread-a'), b = page('thread-b');
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-b', nativeThread('thread-b'));
  // The real Files body must be mounted: leaving its old global URL effect would open A/123 here.
  unregister.push(registerView({ name: 'files', title: 'views.files.title', about: 'views.files.about', icon: File, screens: 'all', area: 'right', body: FilesView }));
  const fixed = makeTab('files'); fixed.scope = { mode: 'fixed', target: { kind: 'thread', id: 'thread-a' } };
  const initialHref = '/thread/thread-b?file=123&line=9';
  b.of = { href: initialHref }; seed([a, b], b.id, [fixed]);
  const drawn = draw(initialHref);
  await settled(drawn, contentHref(fileLocation('environment-thread-b', 'thread-b', '123', 9)));
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file').map((tab) => tab.of)).toEqual([{ env: 'environment-thread-b', thread: 'thread-b', path: '123', line: '9' }]);
  expect(findTab(state().layout, fixed.id)?.tab.scope).toEqual(fixed.scope);
  act(() => { state().change((layout) => setTabTarget(layout, b.id, { href: '/thread/thread-b?file=next.rs&line=27' })); state().focus(b.id); });
  await settled(drawn, contentHref(fileLocation('environment-thread-b', 'thread-b', 'next.rs', 27)));
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file').map((tab) => tab.of?.env)).toEqual(['environment-thread-b', 'environment-thread-b']);
  expect(findTab(state().layout, a.id)).not.toBeNull();
});

it('a late owned native read cannot open a file after its addressed origin closes', async () => {
  const a = page('thread-a'), b = makeTab('page', { href: '/thread/thread-b?file=late.rs&line=5' });
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-b', nativeThread('thread-b')); heldFeeds.add('thread-b');
  seed([a, b], b.id); const drawn = draw(b.of!.href!);
  await waitFor(() => expect(delayedReads.has(b.id)).toBe(true));
  const late = delayedReads.get(b.id)!;
  act(() => state().close({ kind: 'tab', tab: b.id }));
  await act(async () => { late(); await Promise.resolve(); });
  await settled(drawn, a.of!.href);
  expect(allTabs(state().layout).some((tab) => tab.view === 'file' || tab.id === b.id)).toBe(false);
});

it('inactive old links do not steal focus or borrow another pane source while their native response is held', async () => {
  const a = page('thread-a'), b = makeTab('page', { href: '/thread/thread-b?file=waiting.rs' });
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-b', nativeThread('thread-b')); heldFeeds.add('thread-b');
  seed([a, b], a.id); const drawn = draw(a.of!.href!);
  await settled(drawn, a.of!.href); await waitFor(() => expect(delayedReads.has(b.id)).toBe(true));
  act(() => delayedReads.get(b.id)!()); await settled(drawn, a.of!.href);
  expect(state().activeContent).toBe(a.id); expect(allTabs(state().layout).some((tab) => tab.view === 'file')).toBe(false);
  act(() => state().focus(b.id));
  await settled(drawn, contentHref(fileLocation('environment-thread-b', 'thread-b', 'waiting.rs')));
  expect(findTab(state().layout, state().activeContent!)?.tab.of?.env).toBe('environment-thread-b');
});

it('an old reader reply cannot dispatch a file intent after reauthentication; only its fresh owned cut may do so', async () => {
  const a = makeTab('page', { href: '/thread/thread-a?file=reader.rs&line=18' });
  nativeRows.set('thread-a', nativeThread('thread-a')); heldFeeds.add('thread-a'); seed([a], a.id);
  const drawn = draw(a.of!.href!); await waitFor(() => expect(delayedReads.has(a.id)).toBe(true));
  const old = delayedReads.get(a.id)!;
  act(() => client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  act(() => old());
  expect(allTabs(state().layout).some((tab) => tab.view === 'file')).toBe(false);
  const read = contentThreadRead('thread-a', a.id), row = nativeRows.get('thread-a')!;
  act(() => {
    const captured = threadWriter.capture(read, client.session.getState().readerVersion);
    threadWriter.reset(read, { thread: row, items: [], turns: [], segments: [], projection: { viewed_continuation_id: 'active-history', active_continuation_id: 'active-history', follows_active: true, projection_revision: 1, execution_revision: 1 } }, false, captured);
  });
  await settled(drawn, contentHref(fileLocation('environment-thread-a', 'thread-a', 'reader.rs', 18)));
  expect(allTabs(state().layout).filter((tab) => tab.view === 'file')).toHaveLength(1);
});

it.each(['outline', 'given', 'diagnostics'] as const)('following %s inherits historical B, detach retains B after page change/close and never gains controls', async (kind) => {
  const a = page('thread-a', 'history-b'), other = page('thread-other'), panel = makeTab(kind);
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-other', nativeThread('thread-other'));
  seed([a, other], a.id, [panel]); const drawn = draw(a.of!.href!);
  const output = await screen.findByTestId(`inspector-${panel.id}`);
  await waitFor(() => expect(output).toHaveTextContent('history-b'));
  expect(output).toHaveAttribute('data-owner', a.id);
  const before = threadViewOf(contentThreadRead('thread-a', a.id)).read;
  act(() => state().focus(panel.id)); expect(state().activeContent).toBe(a.id);
  expect(threadViewOf(contentThreadRead('thread-a', a.id)).read).toBe(before);
  const group = findTab(state().layout, a.id)!.group.id;
  act(() => state().change((layout) => moveScoped(layout, panel.id, { to: 'split', group, side: 'right' })));
  await waitFor(() => expect(screen.getByTestId(`inspector-${panel.id}`)).toHaveAttribute('data-owner', panel.id));
  expect(findTab(state().layout, panel.id)?.tab.scope).toEqual({ mode: 'fixed', target: { kind: 'thread', id: 'thread-a' }, continuationId: 'history-b' });
  act(() => { state().change((layout) => setTabTarget(layout, a.id, { href: '/thread/thread-a?continuation=history-c' })); state().focus(a.id); });
  await waitFor(() => expect(threadViewOf(contentThreadRead('thread-a', a.id)).read?.selection).toBe('history-c'));
  act(() => state().close({ kind: 'tab', tab: a.id })); await settled(drawn);
  expect(screen.getByTestId(`inspector-${panel.id}`)).toHaveTextContent('history-b');
  expect(screen.getByTestId(`inspector-${panel.id}`)).toHaveAttribute('data-controls', 'false');
  expect(threadViewOf(resultThreadRead('thread-a', panel.id)).read?.selection).toBe('history-b');
  expect(threadViewOf(contentThreadRead('thread-other', other.id)).read?.selection).toBeNull();
});

it('explicit Follow and Change target clear retained history while another owner and composer draft remain intact', async () => {
  const a = page('thread-a', 'history-b'), other = page('thread-other'), panel = makeTab('outline');
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-other', nativeThread('thread-other'));
  seed([a, other], a.id, [panel]); const drawn = draw(a.of!.href!); await settled(drawn);
  await screen.findByRole('textbox', { name: 'Shared-style composer thread-other' });
  fireEvent.change(screen.getByRole('textbox', { name: 'Shared-style composer thread-other' }), { target: { value: 'Keep peer draft' } });
  act(() => fixPanelScope(panel.id));
  await waitFor(() => expect(threadViewOf(resultThreadRead('thread-a', panel.id)).read?.selection).toBe('history-b'));
  act(() => setPanelScope(panel.id, { mode: 'fixed', target: { kind: 'thread', id: 'thread-other' } }));
  await waitFor(() => expect(screen.getByTestId(`inspector-${panel.id}`)).toHaveTextContent('active'));
  expect(findTab(state().layout, panel.id)?.tab.scope).not.toHaveProperty('continuationId');
  act(() => setPanelScope(panel.id, { mode: 'follow' }));
  await waitFor(() => expect(screen.getByTestId(`inspector-${panel.id}`)).toHaveAttribute('data-owner', a.id));
  expect(screen.getByRole('textbox', { name: 'Shared-style composer thread-other' })).toHaveValue('Keep peer draft');
});

it('the actual fixed-history opening hydrator cannot replace shared controls', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The native opening fixture needs its reader.');
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  hydrateThreadOpening(queryClient, opening);
  const before = queryClient.getQueryData(threadKeys.draft(ACME.mainThread));
  const fixed = resultThreadRead(ACME.mainThread, 'retained-panel');
  selectThreadContinuation(fixed, 'history-b');
  const capture = threadWriter.capture(fixed, client.session.getState().readerVersion);
  const controls = queryClient.getQueryData(threadKeys.parameters(ACME.mainThread));
  hydrateThreadOpening(queryClient, { ...opening, draft: null, parameters: null, limits: null, settings: [] }, capture);
  expect(queryClient.getQueryData(threadKeys.draft(ACME.mainThread))).toEqual(before);
  expect(queryClient.getQueryData(threadKeys.parameters(ACME.mainThread))).toEqual(controls);
  expect(queryClient.getQueryData(threadKeys.outline(fixed))).toEqual({ pages: [opening.outline], pageParams: [null] });
  expect(fixed.controls).toBeUndefined();
});

it('shared current workspace metadata cannot replace the existing draft or controls when a file opens', async () => {
  const world = new World(), person = world.people.get('ada');
  if (person === undefined) throw new Error('The native opening fixture needs its reader.');
  const opening = await threadOpening(world, person, ACME.mainThread, 200);
  hydrateThreadOpening(queryClient, opening);
  const draft = queryClient.getQueryData(threadKeys.draft(ACME.mainThread));
  const parameters = queryClient.getQueryData(threadKeys.parameters(ACME.mainThread));
  const source = workspaceThreadRead('workspace-a', ACME.mainThread)!;
  expect(source).toEqual(workspaceThreadRead('workspace-a', ACME.mainThread));
  expect(source).not.toEqual(workspaceThreadRead('workspace-b', ACME.mainThread));
  const capture = threadWriter.capture(source, client.session.getState().readerVersion);
  hydrateThreadOpening(queryClient, { ...opening, draft: null, parameters: null, limits: null, settings: [] }, capture);
  expect(queryClient.getQueryData(threadKeys.draft(ACME.mainThread))).toEqual(draft);
  expect(queryClient.getQueryData(threadKeys.parameters(ACME.mainThread))).toEqual(parameters);
  expect(source.key.kind).toBe('result'); expect(source.controls).toBeUndefined();
});

it.each(['thread', 'document', 'chat'] as const)('a retained legacy main %s loads its ordinary full page under its old local owner', async (kind) => {
  const legacy = makeTab('search-result', { kind, id: `legacy-${kind}`, anchor: 'exact-anchor', continuation: 'history-b', prompt: 'input-b', version: '3' });
  const peer = page('thread-other'); nativeRows.set('legacy-thread', nativeThread('legacy-thread')); nativeRows.set('thread-other', nativeThread('thread-other'));
  seed([legacy, peer], legacy.id);
  const href = contentAddress(findTab(state().layout, legacy.id)!.tab)!;
  const drawn = draw(href); await settled(drawn, href);
  expect(findTab(state().layout, legacy.id)?.tab.view).toBe('page');
  expect(await screen.findByTestId(`full-${kind}-${legacy.id}`)).toBeVisible();
  expect(document.querySelector(`[data-search-result="${legacy.id}"]`)).toBeNull();
  expect(findTab(state().layout, peer.id)).not.toBeNull();
  if (kind === 'thread') expect(screen.getByTestId(`full-thread-${legacy.id}`)).toHaveAttribute('data-selection', 'history-b');
  if (kind === 'document') expect(screen.getByTestId(`full-document-${legacy.id}`)).toHaveAttribute('data-version', '3');
});

it('promoting an already mounted real preview cannot let its old cleanup retire the new full-page B reader', async () => {
  const a = page('thread-a'), preview = makeTab('search-result', { kind: 'thread', id: 'thread-preview', continuation: 'history-b' });
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-preview', nativeThread('thread-preview'));
  seed([a], a.id);
  state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([preview]), open: true } }));
  const drawn = draw(a.of!.href!); await settled(drawn);
  await waitFor(() => expect(threadViewOf(resultThreadRead('thread-preview', preview.id)).read?.selection).toBe('history-b'));
  const previewScope = threadViewOf(resultThreadRead('thread-preview', preview.id)).read;
  act(() => state().change((layout) => moveScoped(layout, preview.id, { to: 'split', group: findTab(layout, a.id)!.group.id, side: 'right' })));
  await screen.findByTestId(`full-thread-${preview.id}`);
  await act(async () => { await Promise.resolve(); });
  const current = threadViewOf(contentThreadRead('thread-preview', preview.id));
  expect(current.thread?.id).toBe('thread-preview'); expect(current.read?.selection).toBe('history-b');
  expect(current.read?.ready).toBe(true); expect(current.read?.controller.signal.aborted).toBe(false);
  expect(previewScope?.controller.signal.aborted).toBe(true);
  expect(current.read?.identity).not.toBe(previewScope?.identity);
  expect(findTab(state().layout, preview.id)?.tab.view).toBe('page');
  expect(findTab(state().layout, a.id)).not.toBeNull();

  // Pins and moves change placement, while the admitted full-page reader keeps its own lifetime.
  act(() => state().change((layout) => setPinned(setPinned(layout, a.id, true), preview.id, true)));
  act(() => state().change((layout) => moveScoped(layout, preview.id, { to: 'group', group: findTab(layout, a.id)!.group.id, index: 0 })));
  await act(async () => { await Promise.resolve(); });
  expect(findTab(state().layout, preview.id)?.group.tabs.map((tab) => tab.id)).toEqual([preview.id, a.id]);
  expect(threadViewOf(contentThreadRead('thread-preview', preview.id)).read).toBe(current.read);
  expect(current.read?.selection).toBe('history-b'); expect(current.read?.ready).toBe(true);
  expect(current.read?.controller.signal.aborted).toBe(false);

  const peer = threadViewOf(contentThreadRead('thread-a', a.id));
  act(() => state().change((layout) => closeTabs(layout, new Set([preview.id]))));
  await act(async () => { await Promise.resolve(); });
  expect(threadViewOf(contentThreadRead('thread-preview', preview.id)).read).toBeNull();
  expect(current.read?.controller.signal.aborted).toBe(true);
  expect(threadViewOf(contentThreadRead('thread-a', a.id))).toBe(peer);
});

it('preview cleanup after the full reader is admitted preserves B and its fresh ticket', async () => {
  const a = page('thread-a'), preview = makeTab('search-result', { kind: 'thread', id: 'thread-handoff', continuation: 'history-b' });
  nativeRows.set('thread-handoff', nativeThread('thread-handoff')); seed([a], a.id);
  state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([preview]), open: true } }));
  const read = contentThreadRead('thread-handoff', preview.id);
  function FullReader() {
    useLayoutEffect(() => selectThreadContinuation(read, 'history-b'), [read]);
    useNativeFeed(read);
    return null;
  }
  function Handoff({ full }: { full: boolean }) {
    useThreadWindowLifetime();
    return <QueryClientProvider client={queryClient}><TooltipProvider>{full ? <FullReader />
      : <ResultThread threadId="thread-handoff" tabId={preview.id} item={null} shown continuationId="history-b" />}</TooltipProvider></QueryClientProvider>;
  }
  const drawn = render(<Handoff full={false} />); mounted.push(drawn);
  await waitFor(() => expect(threadViewOf(read).read?.ready).toBe(true));
  const before = threadViewOf(read).read;
  // Mount the new owner in the same commit, before the preview's deferred cleanup runs.
  act(() => {
    state().change((layout) => moveScoped(layout, preview.id, { to: 'split', group: findTab(layout, a.id)!.group.id, side: 'right' }));
    drawn.rerender(<Handoff full />);
  });
  await act(async () => { await Promise.resolve(); });
  const current = threadViewOf(read);
  expect(before?.controller.signal.aborted).toBe(true);
  expect(current.read?.identity).not.toBe(before?.identity);
  expect(current.thread?.id).toBe('thread-handoff'); expect(current.read?.selection).toBe('history-b');
  expect(current.read?.ready).toBe(true); expect(current.read?.controller.signal.aborted).toBe(false);
});

it.each(['bottom', 'temporary left bar'] as const)('a visible narrow preview in %s admits B through its tab slot without requiring a phone sheet choice', async (area) => {
  const a = page('thread-a'), preview = makeTab('search-result', { kind: 'thread', id: 'thread-narrow-preview', continuation: '' });
  nativeRows.set('thread-a', nativeThread('thread-a')); nativeRows.set('thread-narrow-preview', nativeThread('thread-narrow-preview'));
  seed([a], a.id);
  if (area === 'bottom') state().change((layout) => ({ ...layout, bottom: { ...layout.bottom, group: makeGroup([preview]), open: true } }));
  else {
    state().change((layout) => ({ ...layout, left: { ...layout.left, open: false, active: 'navigation', containers: [{ id: 'preview-bar', name: 'navigation', active: preview.id,
      sections: [{ tab: preview, open: true, hidden: false, size: 1 }] }] } }));
    state().setOverlay('left');
  }
  const width = vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockImplementation(function (this: HTMLElement) {
    return this.dataset.viewInstance === preview.id ? 400 : 1440;
  });
  try {
    const drawn = draw(a.of!.href!); await settled(drawn);
    act(() => state().change((layout) => setTabTarget(layout, preview.id, { kind: 'thread', id: 'thread-narrow-preview', continuation: 'history-b' })));
    await waitFor(() => {
      const current = threadViewOf(resultThreadRead('thread-narrow-preview', preview.id));
      expect(current.read?.selection).toBe('history-b'); expect(current.read?.ready).toBe(true);
    });
    expect(document.querySelector(`[data-search-result="${preview.id}"]`)).toBeVisible();
    expect(feedCalls.some((call) => call.native === 'thread-narrow-preview' && call.owner === preview.id && !call.controls)).toBe(true);
    act(() => state().change((layout) => moveScoped(layout, preview.id, { to: 'split', group: findTab(layout, a.id)!.group.id, side: 'right' })));
    await screen.findByTestId(`full-thread-${preview.id}`);
    const current = threadViewOf(contentThreadRead('thread-narrow-preview', preview.id));
    expect(current.read?.selection).toBe('history-b'); expect(current.read?.ready).toBe(true);
    expect(current.read?.controller.signal.aborted).toBe(false);
  } finally { width.mockRestore(); }
});

it('legacy place-chat uses the actual native chat identity and retains its message through canonical redirect', async () => {
  const world = new World(), person = world.people.get('ada'), row = world.places.rowOf(ACME.fixLogin);
  if (person === undefined || row === undefined) throw new Error('The native place fixture is missing.');
  const node = world.places.topic(person, row); nativeNodes.set(node.id, node);
  const old = makeTab('search-result', { kind: 'place-chat', id: node.id, anchor: 'exact-message' });
  const peer = page('thread-other'); nativeRows.set('thread-other', nativeThread('thread-other')); seed([old, peer], old.id);
  const drawn = draw(contentAddress(findTab(state().layout, old.id)!.tab)!);
  await settled(drawn, `/chat/${node.chat_id}?message=exact-message`);
  expect(await screen.findByTestId(`full-chat-${old.id}`)).toHaveAttribute('data-chat', node.chat_id);
  expect(screen.getByTestId(`full-chat-${old.id}`)).toHaveAttribute('data-message', 'exact-message');
  expect(findTab(state().layout, peer.id)).not.toBeNull();
  expect(allTabs(state().layout).filter((tab) => tab.view === 'page')).toHaveLength(2);
});

it('pending native metadata keeps a remembered real title; actual refusal replaces it and cannot be overwritten by cached names', async () => {
  const a = page('thread-a'); nativeRows.set('thread-a', nativeThread('thread-a')); heldFeeds.add('thread-a'); seed([a], a.id);
  state().setNamed(a.id, { title: 'Grid agent 1', source: 'cache' });
  const drawn = draw(a.of!.href!); await settled(drawn);
  expect(state().named.get(a.id)?.title).toBe('Grid agent 1');
  expect(state().named.get(a.id)?.source).toBe('cache');
  const error = new ApiError({ type: 'orch:error:not_found', code: 'not_found', title: '', detail: 'Synthetic native revoke', status: 404, request_id: 'name-revoke' }, null);
  act(() => threadWriter.failed(contentThreadRead('thread-a', a.id), error));
  await waitFor(() => expect(state().named.get(a.id)?.source).toBe('view'));
  expect(state().named.get(a.id)?.title).not.toBe('Grid agent 1');
  const native = new World().places.rowOf(ACME.mainThread);
  if (native === undefined) throw new Error('The authorized row fixture is missing.');
  act(() => treeWriter.reset('synthetic-tree', [{ ...native, id: 'thread-a', name: 'Stale tree name' }], 1));
  expect(state().named.get(a.id)?.title).not.toBe('Stale tree name');
});

it('an authorized tree can replace a pending generic name, then the identical ready native name takes ownership', async () => {
  const a = page('thread-a'), row = { ...nativeThread('thread-a'), name: 'Grid agent 1' };
  nativeRows.set('thread-a', row); heldFeeds.add('thread-a'); seed([a], a.id);
  const drawn = draw(a.of!.href!); await settled(drawn);
  await waitFor(() => expect(delayedReads.has(a.id)).toBe(true));
  expect(state().named.get(a.id)?.source).not.toBe('view');
  const native = new World().places.rowOf(ACME.mainThread);
  if (native === undefined) throw new Error('The authorized row fixture is missing.');
  act(() => treeWriter.reset('synthetic-tree', [{ ...native, id: 'thread-a', name: row.name }], 1));
  await waitFor(() => expect(state().named.get(a.id)?.title).toBe(row.name));
  expect(state().named.get(a.id)?.source).toBe('cache');
  act(() => { heldFeeds.delete('thread-a'); delayedReads.get(a.id)!(); });
  await waitFor(() => expect(state().named.get(a.id)?.source).toBe('view'));
  expect(state().named.get(a.id)?.title).toBe(row.name);
});
