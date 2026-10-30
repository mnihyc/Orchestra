/** An entry's independent metadata feed cannot revive a refused or replaced reader. */
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type * as ClientModule from '@/api/client';
import { documentTreeEntryKey } from '@/api/documentTree';
import { useTreeEntry } from '@/features/documents/useTreeEntry';
import type { DocumentTreeRead } from '@/generated/api-types';
import type { FeedFrame } from '@/realtime/socket';
const calls = vi.hoisted(() => ({ get: vi.fn<(path: string, options?: unknown) => Promise<DocumentTreeRead>>(), subscribe: vi.fn() }));
vi.mock('@/api/client', async (original) => {
  const actual = await original<typeof ClientModule>();
  return { ...actual, api: { ...actual.api, GET: calls.get }, ok: async (request: Promise<unknown>) => request };
});
vi.mock('@/realtime/live', () => ({ socket: { subscribe: calls.subscribe } }));
const item = { kind: 'document' as const, id: 'document' };
const read = (name: string): DocumentTreeRead => ({ feed: 'entry-feed', ancestors: [], entries: [{
  entry: { item, parent: { kind: 'container', id: 'parent' }, node: 'node', root: 'org', basename: name, title: name,
    owner: 'owner', revision: 1, deleted_at: null, deletion: null, files: 0, versions: 1 },
  standing: { read: true, write: true, manage: false },
}] });
const listeners: { onFrame: (frame: FeedFrame) => void; unsubscribe: ReturnType<typeof vi.fn> }[] = [];
let cache: QueryClient;
function Harness() {
  const value = useTreeEntry(item);
  return <><p>{value.data?.entries[0]?.entry.title ?? 'No entry'}</p>{value.isError && <p>Entry refused</p>}
    <button onClick={() => void value.refetch()}>Refresh</button><button onClick={() => void value.retry()}>Retry</button></>;
}
const mount = () => render(<QueryClientProvider client={cache}><Harness /></QueryClientProvider>);
const deliver = (data: DocumentTreeRead, at = 0) => act(() => listeners[at]?.onFrame({ op: 'event', feed: 'entry-feed', id: at + 1, data: { entry: data } }));
beforeEach(() => {
  cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  calls.get.mockReset(); calls.subscribe.mockReset(); listeners.length = 0;
  calls.subscribe.mockImplementation((_feed: string, listener: { onFrame: (frame: FeedFrame) => void }) => {
    const value = { ...listener, unsubscribe: vi.fn() }; listeners.push(value); return value;
  });
});
afterEach(() => { cleanup(); cache.clear(); });

it('accepts ancestor-only placement changes and rejects a late HTTP cut and another item', async () => {
  calls.get.mockResolvedValueOnce(read('Original')); mount(); await screen.findByText('Original');
  let finish!: (value: DocumentTreeRead) => void;
  calls.get.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(2));
  const next = read('Current entry');
  next.ancestors = [{ id: 'parent', basename: 'Renamed parent', title: 'Renamed parent', node: 'node', root: 'org', parent: { kind: 'node_docs', id: 'node' }, standing: { read: true, write: false, manage: false } }];
  deliver(next); await screen.findByText('Current entry');
  expect(cache.getQueryData(documentTreeEntryKey(item))).toMatchObject({ ancestors: [{ title: 'Renamed parent' }] });
  const wrong = read('Another document');
  const row = wrong.entries[0]; if (row === undefined) throw new Error('Entry required'); row.entry.item = { kind: 'document', id: 'other' };
  deliver(wrong);
  await act(async () => { finish(read('Late HTTP')); await Promise.resolve(); });
  expect(screen.queryByText('Another document')).toBeNull(); expect(screen.queryByText('Late HTTP')).toBeNull();
  expect(listeners).toHaveLength(1);
});

it('removes refused metadata synchronously and ignores later frames even if its view immediately leaves', async () => {
  calls.get.mockResolvedValueOnce(read('Private entry')); const view = mount(); await screen.findByText('Private entry');
  act(() => { listeners[0]?.onFrame({ op: 'closed', feed: 'entry-feed', id: 1, reason: 'revoked' }); view.unmount(); });
  deliver(read('Leaked entry'));
  expect(cache.getQueryData(documentTreeEntryKey(item))).toBeUndefined();
  expect(calls.get).toHaveBeenCalledTimes(1);
});

it('keeps a capacity-waiting subscription and takes its later current metadata without another read', async () => {
  calls.get.mockResolvedValueOnce(read('Original')); mount(); await screen.findByText('Original');
  act(() => listeners[0]?.onFrame({ op: 'error', id: 1, code: 'too_many_subscriptions', detail: 'Waiting' }));
  expect(listeners[0]?.unsubscribe).not.toHaveBeenCalled();
  expect(screen.queryByText('Entry refused')).toBeNull();
  deliver(read('Promoted')); await screen.findByText('Promoted');
  expect(calls.get).toHaveBeenCalledTimes(1);
});

it('a cache-only page shares its frame owner’s read and restarts that owner on explicit retry', async () => {
  function CachedPage() {
    const value = useTreeEntry(item, false, false);
    return <button onClick={() => void value.retry()}>Retry from page</button>;
  }
  calls.get.mockResolvedValueOnce(read('Owned metadata'));
  render(<QueryClientProvider client={cache}><Harness /><CachedPage /></QueryClientProvider>);
  await screen.findByText('Owned metadata'); expect(calls.get).toHaveBeenCalledTimes(1); expect(listeners).toHaveLength(1);
  const previous = listeners[0];
  act(() => previous?.onFrame({ op: 'closed', feed: 'entry-feed', id: 1, reason: 'revoked' }));
  await screen.findByText('Entry refused');
  calls.get.mockResolvedValueOnce(read('Fresh metadata'));
  fireEvent.click(screen.getByRole('button', { name: 'Retry from page' }));
  await screen.findByText('Fresh metadata');
  expect(calls.get).toHaveBeenCalledTimes(2); expect(listeners).toHaveLength(2);
  act(() => previous?.onFrame({ op: 'event', feed: 'entry-feed', id: 1, data: { entry: read('Obsolete metadata') } }));
  expect(screen.queryByText('Obsolete metadata')).toBeNull();
});


it('retires old authority content and delayed reads while the same reader retains only its feed locator', async () => {
  calls.get.mockResolvedValueOnce(read('Private entry')); mount(); await screen.findByText('Private entry');
  let finish!: (value: DocumentTreeRead) => void;
  calls.get.mockReturnValueOnce(new Promise((resolve) => { finish = resolve; }));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh' }));
  await waitFor(() => expect(calls.get).toHaveBeenCalledTimes(2));
  act(() => listeners[0]?.onFrame({ op: 'resync', id: 1, feed: 'entry-feed', reload: true }));
  expect(cache.getQueryData(documentTreeEntryKey(item))).toBeUndefined();
  await screen.findByText('No entry');
  expect(listeners).toHaveLength(2);
  expect(listeners[1]?.unsubscribe).not.toHaveBeenCalled();
  deliver(read('Obsolete frame'), 0);
  await act(async () => { finish(read('Obsolete HTTP')); await Promise.resolve(); });
  expect(screen.queryByText('Obsolete frame')).toBeNull();
  expect(screen.queryByText('Obsolete HTTP')).toBeNull();
  deliver(read('Current authority'), 1); await screen.findByText('Current authority');
  expect(calls.get).toHaveBeenCalledTimes(2);
  expect(listeners).toHaveLength(2);
});
