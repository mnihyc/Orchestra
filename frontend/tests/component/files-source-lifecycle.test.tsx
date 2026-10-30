/** Actual Files/door/tree/ledger lifecycle; only native replies and virtual viewport measurement are synthetic. */
import { act, cleanup, fireEvent, render, screen, waitFor, type RenderResult } from '@testing-library/react';
import { QueryClientProvider } from '@tanstack/react-query';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from '@tanstack/react-router';
import { createStore } from 'zustand/vanilla';
import { File, PanelsTopLeft } from 'lucide-react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as Live from '@/realtime/live';
import type * as Workspace from '@/api/workspace';
import type { DirEntry, DirListing } from '@/generated/api-types';
import type { FeedFrame, SocketStatus } from '@/realtime/socket';
import { ApiError, client } from '@/api/client';
import { queryClient } from '@/api/query';
import { environmentQuery } from '@/api/workspace';
import { FilesView } from '@/features/inspector/Files';
import { allTabs, findTab, makeGroup, makeTab, type Tab } from '@/features/workbench/layout';
import { PageView } from '@/features/workbench/PageView';
import { useWorkbench } from '@/features/workbench/store';
import { clearViewOpenings } from '@/features/workbench/viewOpening';
import { workspaceThreadRead } from '@/features/workbench/workspaceRead';
import { contentThreadRead } from '@/realtime/threadScope';
import { threadWriter } from '@/realtime/stores/thread';
import { directoryFeed, directoryKey, stopWatching, useDirectories } from '@/realtime/stores/environment';
import { clearPageNavigation, pageNavigationOrigin } from '@/features/workbench/pageNavigation';
import { clearScreenHeading, focusScreenHeading } from '@/shell/screen';
import { registerView } from '@/features/workbench/views';
import { TooltipProvider } from '@/ui/Tooltip';
import { Route as ThreadRoute } from '@/routes/thread/$threadId/index';
import { RoutingFrame } from './workbench-routing/frame';
import { delayedReads, feedCalls, FileWitness, heldFeeds, nativeEnvironment, nativeNodes, nativeRows, nativeThread, USER } from './workbench-content/fixture';

const calls = vi.hoisted(() => ({ list: vi.fn(), subscribe: vi.fn() }));
vi.mock('@/realtime/live', async (original) => {
  const actual = await original<typeof Live>();
  const { useNativeFeed } = await import('./workbench-content/fixture');
  return { ...actual, useThreadFeed: useNativeFeed, socket: {
    subscribe: calls.subscribe,
    status: createStore<SocketStatus>(() => ({ phase: 'ready', lostAt: null, roundTripMs: null, build: null, outdated: false })),
  } };
});
vi.mock('@/api/workspace', async (original) => {
  const actual = await original<typeof Workspace>();
  const { nativeEnvironment: environment } = await import('./workbench-content/fixture');
  return { ...actual, listDirectory: calls.list,
    environmentQuery: (id: string) => ({ ...actual.environmentQuery(id), queryFn: () => Promise.resolve({ ...environment(id), workspace_incarnation: 'source-one' }) }),
  };
});
vi.mock('@/features/thread/Thread', async () => {
  const { FullThread } = await import('./workbench-content/fixture'); return { ThreadScreen: FullThread };
});
// Real FileTree interactions and DOM lifetime, with a finite viewport instead of JSDOM layout.
vi.mock('@tanstack/react-virtual', () => ({ useVirtualizer: (options: { count: number; getItemKey: (index: number) => string | number }) => ({
  getTotalSize: () => options.count * 28,
  getVirtualItems: () => Array.from({ length: options.count }, (_, index) => ({ index, key: options.getItemKey(index), start: index * 28, size: 28 })),
  scrollToOffset: () => undefined, scrollToIndex: () => undefined,
}) }));

interface NativeWatch { feed: string; onFrame: (frame: FeedFrame) => void; unsubscribe: ReturnType<typeof vi.fn> }
const watches: NativeWatch[] = [], removes: (() => void)[] = [];
const mounted: RenderResult[] = [], histories: ReturnType<typeof createMemoryHistory>[] = [];
const savedSession = client.session.getState(), savedOptions = queryClient.getDefaultOptions();
const state = () => useWorkbench.getState();
const threadId = 'thread-a', environmentId = 'environment-thread-a';
function entry(path: string, type: 'file' | 'dir'): DirEntry {
  return { path, name: path.slice(path.lastIndexOf('/') + 1), type, size: type === 'file' ? 4 : 0,
    mode: 420, cached: true, mtime: '2026-10-17T00:00:00Z', link_target: null };
}
function listing(path: string): DirListing {
  return { path, entries: path === 'web' ? [entry('web/src', 'dir')]
    : path === 'web/src' ? [entry('web/src/app.ts', 'file'), entry('web/src/lib.ts', 'file')] : [],
    source: 'live', as_of: '2026-10-17T00:00:00Z', mount: null, truncated: false };
}
beforeEach(() => {
  state().leave(); localStorage.clear(); stopWatching(); queryClient.clear(); threadWriter.clear();
  nativeRows.clear(); nativeNodes.clear(); heldFeeds.clear(); delayedReads.clear(); feedCalls.length = 0;
  calls.list.mockReset(); calls.subscribe.mockReset(); watches.length = 0;
  calls.list.mockImplementation((_door: Workspace.Door, path: string) => Promise.resolve(listing(path)));
  calls.subscribe.mockImplementation((feed: string, options: { onFrame: (frame: FeedFrame) => void }) => {
    const watch = { feed, onFrame: options.onFrame, unsubscribe: vi.fn() }; watches.push(watch); return watch;
  });
  client.session.setState({ status: 'signed_in', userId: USER });
  queryClient.setDefaultOptions({ ...savedOptions, queries: { ...savedOptions.queries, retry: false } });
  clearPageNavigation(pageNavigationOrigin()); clearViewOpenings(); focusScreenHeading(null); clearScreenHeading();
  for (const name of ['page', 'screen'] as const) removes.push(registerView({ name, title: 'views.screen.title', about: 'views.screen.about', icon: PanelsTopLeft, screens: 'all', area: 'main', role: 'content', body: PageView }));
  removes.push(registerView({ name: 'file', title: 'files.fileView.title', about: 'files.fileView.about', icon: File, screens: 'all', area: 'main', role: 'content', body: FileWitness }));
  removes.push(registerView({ name: 'files', title: 'views.files.title', about: 'views.files.about', icon: File, screens: 'all', area: 'right', body: FilesView }));
});
afterEach(async () => {
  for (const view of mounted.splice(0)) view.unmount(); cleanup();
  act(() => { state().leave(); stopWatching(); });
  for (const history of histories.splice(0)) history.destroy();
  await queryClient.cancelQueries(); queryClient.clear(); threadWriter.clear();
  for (const remove of removes.splice(0)) remove();
  queryClient.setDefaultOptions(savedOptions); client.session.setState(savedSession); localStorage.clear();
  clearPageNavigation(pageNavigationOrigin()); clearViewOpenings(); focusScreenHeading(null); clearScreenHeading();
});
function draw() {
  nativeRows.set(threadId, { ...nativeThread(threadId), directory: 'web' });
  state().enter(USER, 'thread');
  const page = makeTab('page', { href: `/thread/${threadId}` }), files = makeTab('files');
  state().change((layout) => ({ ...layout, main: makeGroup([page]),
    left: { ...layout.left, open: false, active: null, containers: [] },
    right: { ...layout.right, open: true, active: 'details', containers: [{ id: 'file-group', name: 'details', active: files.id, sections: [{ tab: files, open: true, hidden: false, size: 1 }] }] },
    bottom: { ...layout.bottom, open: false, group: makeGroup() },
  }));
  state().focus(page.id);
  const root = createRootRoute({ component: Outlet, shellComponent: RoutingFrame });
  Object.assign(ThreadRoute.options, { id: '/thread/$threadId/', path: '/thread/$threadId/', getParentRoute: () => root });
  const neutral = createRoute({ getParentRoute: () => root, path: '/workbench', staticData: { screen: 'page' }, component: () => null });
  const history = createMemoryHistory({ initialEntries: [page.of!.href!] }); histories.push(history);
  const router = createRouter({ routeTree: root.addChildren([ThreadRoute, neutral]), history, defaultPendingMs: 0, defaultPendingMinMs: 0 });
  const view = render(<QueryClientProvider client={queryClient}><TooltipProvider><RouterProvider router={router} /></TooltipProvider></QueryClientProvider>);
  mounted.push(view); return { page, files, view };
}
async function expanded() {
  const tree = await screen.findByRole('tree');
  const src = await screen.findByRole('treeitem', { name: /src/ });
  fireEvent.click(src);
  const app = await screen.findByRole('treeitem', { name: /app.ts/ });
  await waitFor(() => expect(src).toHaveAttribute('aria-expanded', 'true'));
  return { tree, src, app };
}
function finishAdmission() {
  const read = workspaceThreadRead(environmentId, threadId)!;
  const deliver = delayedReads.get(read.key.owner);
  if (deliver === undefined) throw new Error('The actual E/T metadata source has not requested its deferred native cut.');
  act(() => { heldFeeds.delete(threadId); deliver(); });
}
function fileTab(): Tab {
  const tab = allTabs(state().layout).find((one) => one.view === 'file');
  if (tab === undefined) throw new Error('The actual file row action did not open its native resource.');
  return tab;
}

it('the actual Board thread-to-file-to-thread transition retains expansion, selection and scroll through a held E/T admission without another tree read', async () => {
  const drawn = draw(), { tree, src, app } = await expanded();
  const sourceWatches = [...watches], reads = calls.list.mock.calls.length;
  tree.scrollTop = 84;
  heldFeeds.add(threadId); fireEvent.click(app);
  const file = fileTab();
  expect(state().activeContent).toBe(file.id);
  await waitFor(() => expect(tree.closest('[inert]')).not.toBeNull());
  expect(tree).not.toBeVisible(); expect(src).toHaveAttribute('aria-expanded', 'true');
  expect(sourceWatches.every((one) => one.unsubscribe.mock.calls.length === 0)).toBe(true);
  // Even a synthetic event that bypasses native inert may not dispatch or retarget a hidden body.
  fireEvent.click(src); expect(src).toHaveAttribute('aria-expanded', 'true');
  expect(calls.list).toHaveBeenCalledTimes(reads);
  finishAdmission();
  await waitFor(() => expect(tree).toBeVisible());
  expect(screen.getByRole('tree')).toBe(tree); expect(tree.scrollTop).toBe(84);
  expect(src).toHaveAttribute('aria-expanded', 'true'); expect(app).toHaveAttribute('aria-selected', 'true');
  expect(watches).toHaveLength(sourceWatches.length); expect(calls.list).toHaveBeenCalledTimes(reads);
  act(() => state().focus(drawn.page.id));
  await waitFor(() => expect(state().activeContent).toBe(drawn.page.id));
  expect(screen.getByRole('tree')).toBe(tree); expect(src).toHaveAttribute('aria-expanded', 'true');
  expect(app).toHaveAttribute('aria-selected', 'true'); expect(tree.scrollTop).toBe(84);
  const workspace = workspaceThreadRead(environmentId, threadId)!;
  expect(feedCalls.some((one) => one.owner === workspace.key.owner && !one.controls)).toBe(true);
  expect(feedCalls.filter((one) => one.native === threadId).every((one) => one.owner === drawn.page.id || one.owner === workspace.key.owner)).toBe(true);
  expect(workspace.controls).toBeUndefined();
});

it('an explicitly chosen folder root survives another same-source admission with its selected child and viewport', async () => {
  draw(); const { src } = await expanded();
  fireEvent.contextMenu(src);
  fireEvent.click(await screen.findByRole('menuitem', { name: 'Open folder here' }));
  const tree = await screen.findByRole('tree');
  await waitFor(() => expect(tree.getAttribute('aria-label')).toContain('web/src'));
  const app = await screen.findByRole('treeitem', { name: /app.ts/ });
  tree.scrollTop = 56; heldFeeds.add(threadId); fireEvent.click(app);
  await waitFor(() => expect(tree.closest('[inert]')).not.toBeNull());
  finishAdmission(); await waitFor(() => expect(tree).toBeVisible());
  expect(screen.getByRole('tree')).toBe(tree); expect(tree.getAttribute('aria-label')).toContain('web/src');
  expect(screen.getByRole('treeitem', { name: /app.ts/ })).toHaveAttribute('aria-selected', 'true');
  expect(tree.scrollTop).toBe(56);
});

it('a terminal source read error releases real watches and cannot leave its old row or a permanent loading tree', async () => {
  const drawn = draw(), { tree } = await expanded(), old = [...watches];
  const source = contentThreadRead(threadId, drawn.page.id);
  act(() => threadWriter.failed(source, new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: 'source-refused' }, null)));
  await screen.findByRole('alert'); expect(tree).not.toBeInTheDocument();
  await waitFor(() => expect(old.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true));
  const count = calls.list.mock.calls.length;
  for (const one of old) act(() => one.onFrame({ op: 'event', id: 1, feed: one.feed, data: { reset: true, changed: [entry('web/private.ts', 'file')], removed: [] } }));
  expect(screen.queryByText('private.ts')).toBeNull(); expect(calls.list).toHaveBeenCalledTimes(count);
});

it('a closed native thread drops its old Files tree instead of serving the retained row as authority', async () => {
  const drawn = draw(), { tree } = await expanded(), old = [...watches];
  act(() => threadWriter.closed(contentThreadRead(threadId, drawn.page.id), 'revoked'));
  await waitFor(() => expect(tree).not.toBeInTheDocument());
  expect(screen.queryByRole('tree')).toBeNull();
  await waitFor(() => expect(old.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true));
});

it('a current native files permission loss removes its admitted tree without reusing saved rights', async () => {
  const drawn = draw(), { tree } = await expanded(), before = [...watches];
  const denied = { ...nativeRows.get(threadId)!, environment_permissions: [] };
  nativeRows.set(threadId, denied);
  const read = contentThreadRead(threadId, drawn.page.id);
  act(() => {
    const capture = threadWriter.capture(read, client.session.getState().readerVersion);
    expect(capture.scope.projection).not.toBeNull();
    expect(threadWriter.reset(read, { thread: denied, items: [], turns: [], segments: [], projection: capture.scope.projection }, false, capture)).not.toBeNull();
  });
  await waitFor(() => expect(tree).not.toBeInTheDocument());
  expect(screen.queryByRole('tree')).toBeNull();
  await waitFor(() => expect(before.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true));
});

it('native directory revocation during a pending admission purges its private rows and cannot be undone by the later metadata cut', async () => {
  draw(); const { tree, app } = await expanded();
  heldFeeds.add(threadId); fireEvent.click(app);
  await waitFor(() => expect(tree.closest('[inert]')).not.toBeNull());
  const root = watches.find((one) => one.feed === directoryFeed({ environmentId, threadId }, 'web'));
  if (root === undefined) throw new Error('The actual web directory native subscription is missing.');
  act(() => root.onFrame({ op: 'closed', id: 1, feed: root.feed, reason: 'revoked' }));
  await waitFor(() => expect(tree).not.toBeInTheDocument());
  finishAdmission(); await screen.findByRole('alert');
  expect(screen.queryByRole('tree')).toBeNull(); expect(app).not.toBeInTheDocument();
  expect(useDirectories.getState().dirs.get(directoryKey(environmentId, 'web'))?.entries).toEqual([]);
});

it('reader and opaque source replacements cannot inherit another authority lifetime or restore its expansion', async () => {
  const drawn = draw(), { tree } = await expanded(), before = [...watches];
  act(() => { queryClient.setQueryData(environmentQuery(environmentId).queryKey, { ...nativeEnvironment(environmentId), workspace_incarnation: 'source-two' }); });
  await waitFor(() => expect(screen.getByRole('tree')).not.toBe(tree));
  const replacement = screen.getByRole('tree');
  await waitFor(() => expect(before.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true));
  expect(await screen.findByRole('treeitem', { name: /src/ })).toHaveAttribute('aria-expanded', 'false');
  const current = [...watches];
  act(() => client.session.setState({ readerVersion: (client.session.getState().readerVersion ?? 0) + 1 }));
  await waitFor(() => expect(replacement).not.toBeInTheDocument());
  expect(current.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true);
  expect(findTab(state().layout, drawn.page.id)).not.toBeNull();
});

it('following a genuinely different E/T cannot preserve the old source tree or receive its late frames', async () => {
  draw(); const { tree } = await expanded(), before = [...watches];
  nativeRows.set('thread-b', { ...nativeThread('thread-b'), directory: 'web' });
  act(() => { state().openTab('page', { href: '/thread/thread-b' }); });
  await waitFor(() => expect(tree).not.toBeInTheDocument());
  const next = await screen.findByRole('tree');
  expect(next).not.toBe(tree);
  expect(await screen.findByRole('treeitem', { name: /src/ })).toHaveAttribute('aria-expanded', 'false');
  await waitFor(() => expect(before.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true));
  expect(calls.list.mock.calls.at(-1)?.[0]).toEqual({ environmentId: 'environment-thread-b', threadId: 'thread-b' });
  for (const one of before) act(() => one.onFrame({ op: 'event', id: 1, feed: one.feed, data: { reset: true, changed: [entry('web/old-source.ts', 'file')], removed: [] } }));
  expect(screen.queryByText('old-source.ts')).toBeNull();
});

it('hiding the sidebar panel during an admission releases its leases, and late metadata and native frames cannot recreate its tree', async () => {
  const drawn = draw(), { app } = await expanded(), before = [...watches];
  heldFeeds.add(threadId); fireEvent.click(app);
  await waitFor(() => expect(delayedReads.has(workspaceThreadRead(environmentId, threadId)!.key.owner)).toBe(true));
  const count = calls.list.mock.calls.length;
  act(() => state().close({ kind: 'tab', tab: drawn.files.id }));
  await waitFor(() => expect(screen.queryByRole('tree')).toBeNull());
  finishAdmission();
  for (const one of before) act(() => one.onFrame({ op: 'event', id: 1, feed: one.feed, data: { reset: true, changed: [entry('web/late.ts', 'file')], removed: [] } }));
  expect(findTab(state().layout, drawn.files.id)?.section?.hidden).toBe(true);
  expect(before.every((one) => one.unsubscribe.mock.calls.length === 1)).toBe(true);
  expect(screen.queryByRole('tree')).toBeNull(); expect(calls.list).toHaveBeenCalledTimes(count);
  expect(useDirectories.getState().dirs.get(directoryKey(environmentId, 'web'))?.entries.map((one) => one.path)).not.toContain('web/late.ts');
});
