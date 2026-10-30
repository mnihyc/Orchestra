/** Directory views keep their page during refresh and never reuse a refused cursor family. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createStore } from 'zustand/vanilla';
import { ApiError } from '@/api/client';
import type * as ClientModule from '@/api/client';
import { documentDirectoryKey } from '@/api/documentTree';
import { useDirectoryPage } from '@/features/documents/useDirectoryPage';
import type { DocumentTreePage } from '@/generated/api-types';
import type { FeedFrame, SocketStatus } from '@/realtime/socket';

const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: unknown) => Promise<DocumentTreePage>>(), subscribe: vi.fn() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: async (request: Promise<unknown>) => request };
});
vi.mock('@/realtime/live', () => ({ socket: { subscribe: calls.subscribe,
  status: createStore<SocketStatus>(() => ({ phase: 'ready', lostAt: null, roundTripMs: null, build: null, outdated: false })) } }));
const request = { node: 'node-a', at: { kind: 'container' as const, id: 'folder' } };
const page = (name: string, node = 'node-a', feed = 'opaque-first'): DocumentTreePage => ({
  items: [], next_cursor: 'next', feed, ancestors: [],
  directory: { at: request.at, node: { id: node, kind: 'project', name: null, redacted: true }, path: [],
    folder: { entry: { item: { kind: 'container', id: 'folder' }, parent: { kind: 'node_docs', id: node }, node, root: 'org',
      basename: name, title: name, revision: 1, owner: 'owner', deleted_at: null, deletion: null, files: 0, versions: 0 },
    standing: { read: true, write: false, manage: false } }, ancestors: [], can_create: false, inspection: null, membership_seq: 1 },
});
const subscriptions: { feed: string; onFrame: (frame: FeedFrame) => void; unsubscribe: ReturnType<typeof vi.fn> }[] = [];
let cache: QueryClient;
function Harness({ node = request.node }: { node?: string }) {
  const view = useDirectoryPage({ ...request, node });
  return <>
    <p>{view.query.data?.directory.folder?.entry.title ?? 'No data'}</p>
    <p data-testid="live">{view.live.phase}</p><p data-testid="anchor">{view.canonicalNode}</p>
    {view.staleCursor && <p>Restart needed</p>}
    <button onClick={() => view.go('next')}>Next</button>
    <button onClick={() => void view.retry().catch(() => undefined)}>Retry</button>
    <button onClick={() => void view.query.refetch().catch(() => undefined)}>Refresh</button>
  </>;
}
const mount = () => render(<QueryClientProvider client={cache}><Harness /></QueryClientProvider>);
const frame = (at: number, data: DocumentTreePage, restart = false) => act(() => {
  const subscription = subscriptions[at];
  if (subscription === undefined) throw new Error('The directory page must be subscribed');
  subscription.onFrame({ op: 'event', id: at + 1, feed: subscription.feed, data: { directory: data, restart } });
});

beforeEach(() => {
  cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  subscriptions.length = 0; calls.get.mockReset(); calls.subscribe.mockReset();
  calls.subscribe.mockImplementation((feed: string, listener: { onFrame: (frame: FeedFrame) => void }) => {
    const subscription = { feed, onFrame: listener.onFrame, unsubscribe: vi.fn() };
    subscriptions.push(subscription); return subscription;
  });
});
afterEach(() => { cleanup(); cache.clear(); });

it('recovers one HTTP stale cursor with one explicit first-page request, including an empty page with more entries', async () => {
  calls.get.mockResolvedValueOnce(page('Original'));
  mount(); await screen.findByText('Original');
  calls.get.mockRejectedValueOnce(new ApiError({ type: 'orch:error:conflict.stale_revision', code: 'conflict.stale_revision',
    status: 409, title: '', detail: '', request_id: '', restart: true, reason: 'document_directory' }, null));
  fireEvent.click(screen.getByRole('button', { name: 'Next' }));
  await screen.findByText('Restart needed');
  expect(calls.get).toHaveBeenCalledTimes(2);
  calls.get.mockResolvedValueOnce(page('Fresh first page'));
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
  await screen.findByText('Fresh first page');
  expect(calls.get).toHaveBeenCalledTimes(3);
  const options: unknown = calls.get.mock.calls.at(-1)?.[1];
  expect(options).toMatchObject({ params: { query: { cursor: null } } });
});

it('keeps its live lease during a refetch and lets current metadata supersede the delayed HTTP cut', async () => {
  calls.get.mockResolvedValueOnce(page('Original')); mount(); await screen.findByText('Original');
  await waitFor(() => expect(subscriptions).toHaveLength(1));
  let resolve!: (data: DocumentTreePage) => void;
  calls.get.mockReturnValueOnce(new Promise<DocumentTreePage>((done) => { resolve = done; }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(2));
  expect(subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  frame(0, page('Current frame'));
  await screen.findByText('Current frame');
  await act(async () => { resolve(page('Obsolete HTTP')); await Promise.resolve(); });
  expect(screen.queryByText('Obsolete HTTP')).toBeNull();
  expect(screen.getByText('Current frame')).toBeVisible();
  expect(subscriptions).toHaveLength(1);
});

it('keeps capacity waiting demand and accepts the promoted snapshot without another HTTP read', async () => {
  calls.get.mockResolvedValueOnce(page('Original')); mount(); await screen.findByText('Original');
  await waitFor(() => expect(subscriptions).toHaveLength(1));
  act(() => subscriptions[0]?.onFrame({ op: 'error', id: 1, code: 'too_many_subscriptions', detail: 'Waiting for a slot' }));
  expect(screen.getByTestId('live')).toHaveTextContent('waiting');
  expect(subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  act(() => subscriptions[0]?.onFrame({ op: 'subscribed', id: 1, feed: 'opaque-first', seq: null, mode: 'ephemeral' }));
  frame(0, page('Promoted'));
  await screen.findByText('Promoted');
  expect(screen.getByTestId('live')).toHaveTextContent('current');
  expect(calls.get).toHaveBeenCalledTimes(1);
});

it('erases previously read cursor pages even when the revoked view leaves immediately', async () => {
  calls.get.mockResolvedValueOnce(page('First private page')); const view = mount(); await screen.findByText('First private page');
  calls.get.mockResolvedValueOnce(page('Second private page', 'node-a', 'opaque-second'));
  fireEvent.click(screen.getByRole('button', { name: 'Next' })); await screen.findByText('Second private page');
  await waitFor(() => expect(subscriptions).toHaveLength(2));
  act(() => {
    subscriptions[1]?.onFrame({ op: 'closed', id: 2, feed: 'opaque-second', reason: 'revoked' });
    view.unmount();
  });
  await act(async () => { await Promise.resolve(); });
  expect(cache.getQueryData(documentDirectoryKey(request))).toBeUndefined();
  expect(cache.getQueryData(documentDirectoryKey({ ...request, cursor: 'next' }))).toBeUndefined();
});

it('follows the moved folder’s canonical anchor without asking to view its hierarchy node', async () => {
  calls.get.mockResolvedValueOnce(page('Original')); mount(); await screen.findByText('Original');
  await waitFor(() => expect(subscriptions).toHaveLength(1));
  frame(0, page('Moved', 'node-b'), true); await screen.findByText('Moved');
  expect(screen.getByTestId('anchor')).toHaveTextContent('node-b');
  expect(calls.get).toHaveBeenCalledTimes(1);
  calls.get.mockResolvedValueOnce(page('Next in moved folder', 'node-b', 'opaque-next'));
  fireEvent.click(screen.getByRole('button', { name: 'Next' })); await screen.findByText('Next in moved folder');
  const options: unknown = calls.get.mock.calls.at(-1)?.[1];
  expect(options).toMatchObject({ params: { path: { id: 'node-b' }, query: { cursor: 'next' } } });
  expect(calls.get.mock.calls.map(([path]) => path)).toEqual(['/nodes/{id}/document-tree', '/nodes/{id}/document-tree']);
});

it('a committed operation resets only its visible directory once and fences an old page and feed', async () => {
  const { applyDocumentTreeChange } = await import('@/api/documentTreeChanges');
  calls.get.mockResolvedValueOnce(page('Original'));
  mount(); await screen.findByText('Original');
  calls.get.mockResolvedValueOnce(page('Second page', 'node-a', 'opaque-second'));
  fireEvent.click(screen.getByRole('button', { name: 'Next' })); await screen.findByText('Second page');
  let finish!: (value: DocumentTreePage) => void;
  calls.get.mockReturnValueOnce(new Promise<DocumentTreePage>((resolve) => { finish = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(3));
  const old = subscriptions.at(-1);
  calls.get.mockResolvedValueOnce(page('Committed first page'));
  const entry = page('New child').directory.folder;
  if (entry === null) throw new Error('The fixture requires a folder');
  await act(async () => {
    await applyDocumentTreeChange(cache, { input: { command: 'create_folder', client_id: 'create', items: [],
      to: request.at, name: 'Child', version: null, deletion_plan: null },
      receipt: { operation: 'operation', entries: [entry.entry], copied: [], gaining: 0, losing: 0, restore_until: null,
        view: { entries: [], ancestors: [] } } });
  });
  await screen.findByText('Committed first page');
  await act(async () => {
    finish(page('Obsolete HTTP'));
    old?.onFrame({ op: 'event', id: 1, feed: old.feed, data: { directory: page('Obsolete live page') } });
    await Promise.resolve();
  });
  expect(screen.queryByText('Obsolete HTTP')).toBeNull();
  expect(screen.queryByText('Obsolete live page')).toBeNull();
  expect(calls.get).toHaveBeenCalledTimes(4);
  expect(calls.get.mock.calls.at(-1)?.[1]).toMatchObject({ params: { query: { cursor: null } } });
});


it('repairs a moved directory with the current receipt anchor, without an old-node request', async () => {
  const { applyDocumentTreeChange } = await import('@/api/documentTreeChanges');
  calls.get.mockResolvedValueOnce(page('Original')); mount(); await screen.findByText('Original');
  const moved = page('Moved', 'node-b').directory.folder;
  if (moved === null) throw new Error('The current folder is required');
  calls.get.mockResolvedValueOnce(page('Moved current page', 'node-b'));
  await act(async () => {
    await applyDocumentTreeChange(cache, { input: { command: 'move', client_id: 'move', items: [moved.entry.item],
      to: { kind: 'node_docs', id: 'node-b' }, name: null, version: null, deletion_plan: null },
      receipt: { operation: 'move-operation', entries: [moved.entry], copied: [], gaining: 0, losing: 0, restore_until: null,
        view: { entries: [moved], ancestors: [] } } });
  });
  await screen.findByText('Moved current page');
  expect(screen.getByTestId('anchor')).toHaveTextContent('node-b');
  expect(calls.get).toHaveBeenCalledTimes(2);
  expect(calls.get.mock.calls[1]?.[1]).toMatchObject({ params: { path: { id: 'node-b' }, query: { cursor: null } } });
});


it('a newer typed entry anchor supersedes an older live reanchor and its cursor', async () => {
  calls.get.mockResolvedValueOnce(page('Original')); const view = mount(); await screen.findByText('Original');
  frame(0, page('Earlier live move', 'node-b'), true); await screen.findByText('Earlier live move');
  calls.get.mockResolvedValueOnce(page('Page two at B', 'node-b', 'opaque-b-next'));
  fireEvent.click(screen.getByRole('button', { name: 'Next' })); await screen.findByText('Page two at B');
  const previous = subscriptions.at(-1);
  calls.get.mockResolvedValueOnce(page('Current entry anchor', 'node-c', 'opaque-c'));
  view.rerender(<QueryClientProvider client={cache}><Harness node="node-c" /></QueryClientProvider>);
  await screen.findByText('Current entry anchor');
  expect(screen.getByTestId('anchor')).toHaveTextContent('node-c');
  expect(calls.get.mock.calls.at(-1)?.[1]).toMatchObject({ params: { path: { id: 'node-c' }, query: { cursor: null } } });
  act(() => previous?.onFrame({ op: 'event', id: 2, feed: 'opaque-b-next', data: { directory: page('Old B frame', 'node-b') } }));
  expect(screen.queryByText('Old B frame')).toBeNull();
  expect(calls.get).toHaveBeenCalledTimes(3);
});


it('retires the authority cursor family without losing the same reader’s replacement feed locator', async () => {
  calls.get.mockResolvedValueOnce(page('Private first page')); mount(); await screen.findByText('Private first page');
  calls.get.mockResolvedValueOnce(page('Private second page', 'node-a', 'opaque-second'));
  fireEvent.click(screen.getByRole('button', { name: 'Next' })); await screen.findByText('Private second page');
  await waitFor(() => expect(subscriptions).toHaveLength(2));
  let finish!: (value: DocumentTreePage) => void;
  calls.get.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(3));
  act(() => subscriptions[1]?.onFrame({ op: 'resync', id: 2, feed: 'opaque-second', reload: true }));
  expect(cache.getQueryData(documentDirectoryKey(request))).toBeUndefined();
  expect(cache.getQueryData(documentDirectoryKey({ ...request, cursor: 'next' }))).toBeUndefined();
  await screen.findByText('No data');
  expect(subscriptions).toHaveLength(3);
  expect(subscriptions[2]?.unsubscribe).not.toHaveBeenCalled();
  frame(1, page('Obsolete frame', 'node-a', 'opaque-second'));
  await act(async () => { finish(page('Obsolete HTTP', 'node-a', 'opaque-second')); await Promise.resolve(); });
  expect(screen.queryByText('Obsolete frame')).toBeNull();
  expect(screen.queryByText('Obsolete HTTP')).toBeNull();
  frame(2, page('Current authority', 'node-a', 'opaque-second')); await screen.findByText('Current authority');
  expect(calls.get).toHaveBeenCalledTimes(3);
  expect(subscriptions).toHaveLength(3);
});
