/** Recent draws permission-filtered pages without per-row reads and retains list loading/failure states. */
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from '@tanstack/react-router';
import { QueryClientProvider } from '@tanstack/react-query';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { RecentList } from '@/features/navigation/RecentList';
import { navigationFilter } from '@/features/navigation/recent';
import type { EverythingRowPage } from '@/generated/api-types';
import { i18n, setLocale } from '@/i18n';
import { ApiError } from '@/api/client';
import { LiveQueryClient } from '@/api/query';
import { tellUserEvent } from '@/realtime/stores/user';
import { TooltipProvider } from '@/ui/Tooltip';
import { Toaster } from '@/ui/Toast';
import { createContext, useContext, type ReactNode } from 'react';
import { inventoryRow } from '../doubles/everything-row';
import { acmeTree, ACME } from '../doubles/fixtures';
import { display } from '../doubles/thread-display';

const ContentContext = createContext<ReactNode>(null);
function Content() { return <>{useContext(ContentContext)}</>; }

function show(ui: ReactNode) {
  const root = createRootRoute({ component: Content });
  const thread = createRoute({ getParentRoute: () => root, path: '/thread/$threadId', component: () => null });
  const router = createRouter({ routeTree: root.addChildren([thread]), history: createMemoryHistory({ initialEntries: ['/'] }) });
  const cache = new LiveQueryClient({ defaultOptions: { queries: { retry: false } } });
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={cache}><TooltipProvider><Toaster><ContentContext.Provider value={children}><RouterProvider router={router} /></ContentContext.Provider></Toaster></TooltipProvider></QueryClientProvider>;
  return render(ui, { wrapper });
}

const read = vi.hoisted(() => vi.fn<() => Promise<EverythingRowPage>>());
const open = vi.hoisted(() => vi.fn());
vi.mock('@/api/everything', () => ({ everythingQuery: (reader: string, parent: string, filter: unknown) => ({ queryKey: ['everything', reader, parent, filter], initialPageParam: null, queryFn: () => read(), getNextPageParam: (page: EverythingRowPage) => page.next_cursor }) }));
vi.mock('@/features/chat/open', () => ({ useOpenPath: () => open }));
vi.mock('@/features/workbench/viewOpening', () => ({ useViewShown: () => true }));

const tree = { nodes: new Map(), lastSeq: 1, loaded: true, error: null };
const filter = navigationFilter('opened', '', { threads: true, documents: true, chats: false }, false, false, false);
const props = { reader: 'reader', orgId: 'org', parent: 'project', tree, filter, current: null, needs: () => ({ unread: false, waiting: 0, mentions: 0 }), onShowTree: vi.fn() };
const place = { id: 'project', kind: 'project' as const, name: 'Web', redacted: false };
const doc = (id: string, title: string) => inventoryRow({ id, kind: 'document', name: title, path: [place], parent_id: 'project', place_label: 'Acme / Web', href: `/doc/${id}`, last_opened: new Date().toISOString(), document_unread: true });

beforeEach(() => { read.mockReset(); open.mockReset(); props.onShowTree.mockReset(); });
afterEach(async () => { cleanup(); await setLocale('en'); });

it('waits for the shared tree and then lists many rows with one read and the reader’s visit times', async () => {
  const rows = Array.from({ length: 50 }, (_, index) => doc(`doc-${index}`, `Notes ${index}`));
  read.mockResolvedValue({ items: rows, next_cursor: null });
  const view = show(<RecentList {...props} tree={{ ...tree, loaded: false }} />);
  await act(async () => { await Promise.resolve(); });
  expect(read).not.toHaveBeenCalled();
  view.rerender(<RecentList {...props} />);
  expect(await screen.findByRole('heading', { name: 'Today' })).toBeVisible();
  expect(screen.getAllByRole('treeitem')).toHaveLength(50);
  expect(read).toHaveBeenCalledTimes(1);
  expect(screen.getAllByText('Web')).toHaveLength(50);
  expect(document.querySelectorAll('time[datetime]')).toHaveLength(50);
  expect(screen.getByText('Notes 0')).toHaveClass('font-semibold');
  await userEvent.click(screen.getByRole('link', { name: i18n.t('everything.placedName', { name: 'Notes 0', place: 'Acme / Web' }) }));
  expect(open).toHaveBeenCalledWith('/doc/doc-0');
});

it('keeps empty, narrowed-empty and failure/retry in the list’s own space', async () => {
  read.mockRejectedValueOnce(new Error('Disconnected')).mockResolvedValue({ items: [], next_cursor: null });
  const user = userEvent.setup();
  const view = show(<RecentList {...props} />);
  await user.click(await screen.findByRole('button', { name: 'Try again' }));
  await screen.findByText(i18n.t('navigation.noRecent'));
  expect(read).toHaveBeenCalledTimes(2);
  view.rerender(<RecentList {...props} filter={{ ...filter, q: 'missing' }} />);
  await screen.findByText(i18n.t('navigation.filter.none'));
  expect(screen.queryByRole('treeitem')).not.toBeInTheDocument();
});

it('drops a formerly readable row when the shared tree advances and the next authorized page omits it', async () => {
  read.mockResolvedValueOnce({ items: [doc('gone', 'Gone notes')], next_cursor: null }).mockResolvedValue({ items: [], next_cursor: null });
  const view = show(<RecentList {...props} />);
  await screen.findByText('Gone notes');
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 2 }} />);
  await waitFor(() => expect(screen.queryByText('Gone notes')).not.toBeInTheDocument());
  expect(read).toHaveBeenCalledTimes(2);
});

it('names Active’s empty state without claiming that the reader never opened anything', async () => {
  read.mockResolvedValue({ items: [], next_cursor: null });
  show(<RecentList {...props} filter={{ ...filter, recent: 'active', sort: 'last_activity' }} />);
  expect(await screen.findByText(i18n.t('navigation.noActivity'))).toBeVisible();
  expect(screen.queryByText(i18n.t('navigation.noRecent'))).not.toBeInTheDocument();
});

it('retains a held list after a network failure and removes it after an authoritative access refusal', async () => {
  read.mockResolvedValueOnce({ items: [doc('held', 'Held notes')], next_cursor: null }).mockRejectedValueOnce(new Error('Disconnected'))
    .mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: 'Not allowed', status: 403, detail: 'No longer readable', request_id: 'recent-cut' }, null))
    .mockRejectedValueOnce(new Error('Still disconnected')).mockResolvedValue({ items: [doc('held', 'Restored notes')], next_cursor: null });
  const view = show(<RecentList {...props} />);
  await screen.findByText('Held notes');
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 2 }} />);
  await screen.findByRole('button', { name: 'Try again' });
  expect(screen.getByText('Held notes')).toBeVisible();
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 3 }} />);
  await waitFor(() => expect(screen.queryByText('Held notes')).not.toBeInTheDocument());
  expect(read).toHaveBeenCalledTimes(3);
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 4 }} />);
  await waitFor(() => expect(read).toHaveBeenCalledTimes(4));
  expect(screen.queryByText('Held notes')).not.toBeInTheDocument();
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 5 }} />);
  expect(await screen.findByText('Restored notes')).toBeVisible();
  expect(read).toHaveBeenCalledTimes(5);
});

it('refreshes Opened from the existing visit feed under the native exact-query policy', async () => {
  read.mockResolvedValueOnce({ items: [doc('before', 'Before visit')], next_cursor: null })
    .mockResolvedValue({ items: [doc('after', 'After visit')], next_cursor: null });
  show(<RecentList {...props} />);
  await screen.findByText('Before visit');
  act(() => tellUserEvent('recents', {}));
  expect(await screen.findByText('After visit')).toBeVisible();
  expect(screen.queryByText('Before visit')).not.toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(2);
});

it.each(['waiting_approval', 'blocked', 'failed', 'running', 'queued', 'held', 'finished', 'idle', 'archived'] as const)('uses the supplied %s state in a Recent row without a detail read', async (state) => {
  const row = inventoryRow({ id: 'recent-thread', kind: 'thread', name: 'A recent thread', href: '/thread/recent-thread',
    path: [place], place_label: 'Acme / Web', last_opened: new Date().toISOString(), state: 'idle',
    thread_display: display(state, { queued_inputs: 2, held_inputs: 2 }) });
  read.mockResolvedValue({ items: [row], next_cursor: null });
  const view = show(<RecentList {...props} />);
  await screen.findByRole('link', { name: 'A recent thread · Acme / Web' });
  expect(view.container.querySelector(`[data-work-state="${state}"]`)).toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(1);
});


it.each(['en', 'zh-CN'] as const)('orders authorized running state across tree and Recent without extra reads in %s', async (locale) => {
  await setLocale(locale);
  const base = acmeTree().find((node) => node.id === ACME.mainThread);
  if (base === undefined || base.thread == null) throw new Error('The fixture has a readable thread');
  const node = { ...base, id: 'recent-thread', name: 'A recent thread', thread: { ...base.thread, display: display('idle', { seq: 10 }) } };
  const row = inventoryRow({ id: node.id, kind: 'thread', name: node.name, href: `/thread/${node.id}`, path: [place], last_opened: new Date().toISOString(), thread_display: display('running', { seq: 20 }) });
  read.mockResolvedValue({ items: [row], next_cursor: null });
  const view = show(<RecentList {...props} tree={{ ...tree, nodes: new Map([[node.id, node]]) }} />);
  await screen.findByText(node.name);
  expect(view.container.querySelector('[data-work-state="running"]')).toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(1);
  const queued = { ...node, thread: { ...node.thread, display: display('queued', { seq: 30, queued_inputs: 1 }) } };
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 2, nodes: new Map([[node.id, queued]]) }} />);
  await waitFor(() => expect(view.container.querySelector('[data-work-state="queued"]')).toBeInTheDocument());
  expect(view.container.querySelector('[data-work-state="running"]')).toBeNull();
  const finished = { ...node, thread: { ...node.thread, display: display('finished', { seq: 40 }) } };
  view.rerender(<RecentList {...props} tree={{ ...tree, lastSeq: 3, nodes: new Map([[node.id, finished]]) }} />);
  await waitFor(() => expect(view.container.querySelector('[data-work-state="finished"]')).toBeInTheDocument());
  expect(view.container.querySelector('[data-work-state="running"]')).toBeNull();
});

it.each(['en', 'zh-CN'] as const)('keeps a path-only Recent row plain even while its old tree node has private running facts in %s', async (locale) => {
  await setLocale(locale);
  const base = acmeTree().find((node) => node.id === ACME.mainThread);
  if (base === undefined || base.thread == null) throw new Error('The fixture has a readable thread');
  const node = { ...base, id: 'recent-thread', name: 'A recent thread', thread: { ...base.thread, display: display('running', { seq: 50 }) } };
  const row = inventoryRow({ id: node.id, kind: 'thread', name: node.name, my_access: 'path_only', href: `/thread/${node.id}`, path: [place], last_opened: new Date().toISOString() });
  read.mockResolvedValue({ items: [row], next_cursor: null });
  const view = show(<RecentList {...props} tree={{ ...tree, nodes: new Map([[node.id, node]]) }} />);
  await screen.findByText(node.name);
  expect(view.container.querySelector('[data-work-state]')).toBeNull();
  expect(screen.queryByRole('img', { name: i18n.t('threadStates.notReported') })).not.toBeInTheDocument();
  expect(view.container.querySelector('svg.lucide-message-square-text[aria-hidden="true"]')).toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(1);
});

it('keeps a newer personal read mark when a delayed listing has the same work sequence', async () => {
  const base = acmeTree().find((node) => node.id === ACME.mainThread);
  if (base === undefined || base.thread == null) throw new Error('The fixture has a readable thread');
  const node = { ...base, id: 'recent-thread', name: 'A recent thread', thread: { ...base.thread, display: display('idle', { seq: 50, read_seq: 4 }) } };
  const row = inventoryRow({ id: node.id, kind: 'thread', name: node.name, href: `/thread/${node.id}`, path: [place], last_opened: new Date().toISOString(), thread_display: display('finished', { seq: 50, read_seq: 3 }) });
  read.mockResolvedValue({ items: [row], next_cursor: null });
  const view = show(<RecentList {...props} tree={{ ...tree, nodes: new Map([[node.id, node]]) }} />);
  await screen.findByText(node.name);
  expect(view.container.querySelector('[data-work-state="idle"]')).toBeInTheDocument();
  expect(view.container.querySelector('[data-work-state="finished"]')).toBeNull();
});
