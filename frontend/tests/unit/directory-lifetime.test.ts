/** Delayed native reads, authority changes and directory demand must not corrupt a visible tree. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { createStore } from 'zustand/vanilla';
import { ApiError, client } from '@/api/client';
import type { DirEntry, DirListing } from '@/generated/api-types';
import type { FeedFrame, SocketStatus } from '@/realtime/socket';
import { socket } from '@/realtime/live';
import { DIRECTORY_READ_CONCURRENCY, MOST_WATCHED, directoryKey, directoryWriter, sortEntries, stopWatching, useDirectories, watchDirectory } from '@/realtime/stores/environment';

const calls = vi.hoisted(() => ({ list: vi.fn(), subscribe: vi.fn() }));
vi.mock('@/api/workspace', () => ({ listDirectory: calls.list }));
vi.mock('@/realtime/live', () => ({ socket: { subscribe: calls.subscribe, status: createStore<SocketStatus>(() => ({ phase: 'ready', lostAt: null, roundTripMs: null, build: null, outdated: false })) } }));
const door = { environmentId: 'workspace', threadId: 'thread' };
const subscriptions: { onFrame: (frame: FeedFrame) => void; unsubscribe: ReturnType<typeof vi.fn> }[] = [];
const entry = (name: string): DirEntry => ({ name, path: name, type: 'file', size: 1, mtime: '2026-01-01T00:00:00Z', cached: false, mode: 420, link_target: null });
const listing = (name: string, path = ''): DirListing => ({ entries: [entry(name)], path, mount: null, source: 'live', as_of: '2026-01-01T00:00:00Z', truncated: false });
const held = (path = '') => useDirectories.getState().dirs.get(directoryKey(door.environmentId, path));
const update = (name: string): FeedFrame => ({ op: 'event', id: 1, feed: 'directory', data: { reset: true, changed: [entry(name)], removed: [], truncated: false } });
function pending<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  stopWatching(); subscriptions.length = 0; calls.list.mockReset(); calls.subscribe.mockReset();
  socket.status.setState({ phase: 'ready' });
  calls.subscribe.mockImplementation((_feed: string, options: { onFrame: (frame: FeedFrame) => void }) => {
    const sub = { onFrame: options.onFrame, unsubscribe: vi.fn() }; subscriptions.push(sub); return sub;
  });
});
afterEach(() => { stopWatching(); });

it('does not replace a newer directory reset with an older HTTP snapshot', async () => {
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const load = directoryWriter.load(door, ''); const release = watchDirectory(door, '');
  subscriptions[0]?.onFrame(update('current.ts'));
  old.resolve(listing('old.ts')); await load;
  expect(held()?.entries.map((one) => one.name)).toEqual(['current.ts']);
  expect(held()?.watch).toBe('live'); release();
});

it('keeps a cached fallback that finishes after an offline notice without calling it live', async () => {
  const response = pending<DirListing>(); calls.list.mockReturnValueOnce(response.promise);
  const release = watchDirectory(door, ''); const load = directoryWriter.load(door, '');
  subscriptions[0]?.onFrame({ op: 'event', id: 1, feed: 'directory', data: { offline: true } });
  response.resolve({ ...listing('cached.ts'), source: 'cached' }); await load;
  expect(held()).toMatchObject({ status: 'ready', source: 'cached', offline: true, watch: 'offline' });
  expect(held()?.entries[0]?.name).toBe('cached.ts'); release();
});

it('retires both late HTTP and old socket frames when the reader changes', async () => {
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const load = directoryWriter.load(door, ''); const release = watchDirectory(door, '');
  const previous = subscriptions[0];
  client.session.setState((state) => ({ readerVersion: (state.readerVersion ?? 0) + 1 }));
  expect(previous?.unsubscribe).toHaveBeenCalledTimes(1);
  previous?.onFrame(update('private.ts')); old.resolve(listing('private.ts')); await load; release();
  expect(held()).toBeUndefined();
  calls.list.mockResolvedValueOnce(listing('new-reader.ts'));
  await directoryWriter.load(door, ''); expect(held()?.entries[0]?.name).toBe('new-reader.ts');
});

it('removes names on revocation and cannot restore them through an already pending read', async () => {
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const release = watchDirectory(door, ''); subscriptions[0]?.onFrame(update('private.ts'));
  const load = directoryWriter.load(door, '');
  subscriptions[0]?.onFrame({ op: 'closed', id: 1, feed: 'directory', reason: 'revoked' });
  expect(held()?.entries).toEqual([]); expect(held()?.watch).toBe('refused');
  subscriptions[0]?.onFrame(update('late-private.ts'));
  old.resolve(listing('private.ts')); await load; expect(held()?.entries).toEqual([]); release();
});

it('shares stable watches, enforces the budget and promotes waiting demand without HTTP polling', () => {
  const first = watchDirectory(door, 'same'); const second = watchDirectory(door, 'same');
  expect(subscriptions).toHaveLength(1); first(); expect(subscriptions[0]?.unsubscribe).not.toHaveBeenCalled();
  const releases = Array.from({ length: MOST_WATCHED }, (_, index) => watchDirectory(door, `folder-${index}`));
  expect(subscriptions).toHaveLength(MOST_WATCHED);
  expect(held(`folder-${MOST_WATCHED - 1}`)?.watch).toBe('waiting');
  second(); expect(subscriptions[0]?.unsubscribe).toHaveBeenCalledTimes(1);
  expect(subscriptions).toHaveLength(MOST_WATCHED + 1);
  expect(held(`folder-${MOST_WATCHED - 1}`)?.watch).toBe('connecting');
  expect(calls.list).not.toHaveBeenCalled();
  for (const release of releases) release();
  expect(subscriptions.every((sub) => sub.unsubscribe.mock.calls.length === 1)).toBe(true);
});

it('ignores an old subscription after resync and waits for the replacement reset', () => {
  const release = watchDirectory(door, ''); const old = subscriptions[0];
  old?.onFrame(update('before.ts'));
  old?.onFrame({ op: 'resync', id: 1, feed: 'directory' });
  expect(held()?.watch).toBe('connecting'); expect(subscriptions).toHaveLength(2);
  subscriptions[1]?.onFrame(update('after.ts')); old?.onFrame(update('stale.ts'));
  expect(held()?.entries[0]?.name).toBe('after.ts'); release();
});

it('bounds concurrent native reads and cancels queued work before it can start for an old reader', async () => {
  const gates = Array.from({ length: DIRECTORY_READ_CONCURRENCY }, () => pending<DirListing>());
  for (const gate of gates) calls.list.mockReturnValueOnce(gate.promise);
  const loads = Array.from({ length: DIRECTORY_READ_CONCURRENCY + 3 }, (_, index) => directoryWriter.load(door, String(index)));
  expect(calls.list).toHaveBeenCalledTimes(DIRECTORY_READ_CONCURRENCY);
  directoryWriter.clear();
  for (const gate of gates) gate.resolve(listing('retired.ts'));
  await Promise.all(loads);
  expect(calls.list).toHaveBeenCalledTimes(DIRECTORY_READ_CONCURRENCY);
  expect(useDirectories.getState().dirs.size).toBe(0);
});

it('does not let a retired environment response overwrite its replacement', async () => {
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const load = directoryWriter.load(door, ''); directoryWriter.forget(door.environmentId);
  calls.list.mockResolvedValueOnce(listing('new-binding.ts')); await directoryWriter.load(door, '');
  old.resolve(listing('old-binding.ts')); await load;
  expect(held()?.entries[0]?.name).toBe('new-binding.ts');
});

it('cancels hidden queued reads while retaining the one directory another visible owner still needs', async () => {
  const gates = Array.from({ length: DIRECTORY_READ_CONCURRENCY }, () => pending<DirListing>());
  for (const gate of gates) calls.list.mockReturnValueOnce(gate.promise);
  calls.list.mockResolvedValueOnce(listing('shared.ts', '4'));
  const releases = Array.from({ length: 7 }, (_, index) => watchDirectory(door, String(index)));
  const shared = watchDirectory(door, '4');
  const loads = releases.map((_, index) => directoryWriter.load(door, String(index), { visible: true }));
  expect(calls.list).toHaveBeenCalledTimes(4);
  for (const release of releases) release();
  for (const gate of gates) gate.resolve(listing('retired.ts'));
  await Promise.all(loads);
  expect(calls.list).toHaveBeenCalledTimes(5);
  expect(calls.list.mock.calls.at(-1)?.[1]).toBe('4');
  expect(held('4')?.entries[0]?.name).toBe('shared.ts'); shared();
  const fresh = watchDirectory(door, '5'); calls.list.mockResolvedValueOnce(listing('reopened.ts', '5'));
  await directoryWriter.load(door, '5', { visible: true });
  expect(calls.list).toHaveBeenCalledTimes(6); expect(held('5')?.entries[0]?.name).toBe('reopened.ts'); fresh();
});

it('retires freshness on raw transport loss and requires data after reconnect', () => {
  const release = watchDirectory(door, ''); subscriptions[0]?.onFrame(update('kept.ts'));
  expect(held()?.watch).toBe('live'); socket.status.setState({ phase: 'waiting' });
  expect(held()?.watch).toBe('offline'); expect(held()?.entries[0]?.name).toBe('kept.ts');
  socket.status.setState({ phase: 'ready' }); expect(held()?.watch).toBe('connecting');
  subscriptions[0]?.onFrame({ op: 'subscribed', id: 2, feed: 'directory', mode: 'ephemeral', seq: null });
  expect(held()?.watch).toBe('connecting'); subscriptions[0]?.onFrame(update('current.ts'));
  expect(held()?.watch).toBe('live'); expect(calls.list).not.toHaveBeenCalled(); release();
});

it('forgets an active watch without poisoning its replacement or retaining its budget slot', () => {
  const oldRelease = watchDirectory(door, ''); const old = subscriptions[0]; old?.onFrame(update('old.ts'));
  directoryWriter.forget(door.environmentId); expect(old?.unsubscribe).toHaveBeenCalledTimes(1);
  const release = watchDirectory(door, ''); expect(subscriptions).toHaveLength(2);
  subscriptions[1]?.onFrame(update('new.ts')); old?.onFrame(update('retired.ts')); oldRelease();
  expect(held()?.entries[0]?.name).toBe('new.ts'); expect(subscriptions[1]?.unsubscribe).not.toHaveBeenCalled(); release();
});

it('keeps native activation failure visible over cached rows and retries only that installation', async () => {
  const release = watchDirectory(door, '');
  const first = subscriptions[0];
  first?.onFrame(update('retained.ts'));
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const load = directoryWriter.load(door, '');
  first?.onFrame({ op: 'event', id: 1, feed: 'directory', data: {
    error: { code: 'environment.unavailable', reason: 'busy', detail: 'The directory watch limit is full.', retryable: true },
  } });
  old.resolve(listing('obsolete.ts')); await load;
  first?.onFrame(update('late.ts'));
  expect(held()).toMatchObject({ status: 'failed', watch: 'failed', watchError: { reason: 'busy' } });
  expect(held()?.entries.map((item) => item.name)).toEqual(['retained.ts']);
  calls.list.mockResolvedValueOnce(listing('fresh.ts'));
  await directoryWriter.retry(door, '');
  expect(subscriptions).toHaveLength(2);
  expect(first?.unsubscribe).toHaveBeenCalledTimes(1);
  expect(held()?.watch).toBe('connecting');
  subscriptions[1]?.onFrame(update('fresh.ts'));
  expect(held()).toMatchObject({ status: 'ready', watch: 'live', watchError: null });
  expect(calls.list).toHaveBeenCalledTimes(2); release();
});

it('clears inaccessible HTTP rows and ignores old acknowledgements until a successful explicit retry', async () => {
  const release = watchDirectory(door, '');
  const first = subscriptions[0]; first?.onFrame(update('private.ts'));
  calls.list.mockRejectedValueOnce(new ApiError({ type: 'orch:error:authz.denied', code: 'authz.denied', title: '', detail: '', status: 403, request_id: '' }, null));
  await directoryWriter.load(door, '');
  first?.onFrame({ op: 'subscribed', id: 1, feed: 'directory', seq: null, mode: 'ephemeral' });
  first?.onFrame(update('late-private.ts'));
  expect(held()?.entries).toEqual([]);
  expect(held()?.watch).toBe('refused');
  calls.list.mockResolvedValueOnce(listing('authorized-again.ts'));
  await directoryWriter.retry(door, '');
  expect(subscriptions).toHaveLength(2);
  first?.onFrame(update('still-obsolete.ts'));
  subscriptions[1]?.onFrame(update('authorized-again.ts'));
  expect(held()?.entries[0]?.name).toBe('authorized-again.ts'); release();
});

it('invalidates a complete page identity without polling or accepting an older in-flight snapshot', async () => {
  const release = watchDirectory(door, '');
  subscriptions[0]?.onFrame({ op: 'event', id: 1, feed: 'directory', data: {
    reset: true, changed: [entry('page-one.ts')], removed: [], truncated: true, revision: 'v1', next_cursor: 'next-v1',
  } });
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const load = directoryWriter.load(door, '');
  subscriptions[0]?.onFrame({ op: 'event', id: 1, feed: 'directory', data: { invalidated: true, changed: [], removed: [] } });
  old.resolve({ ...listing('obsolete.ts'), revision: 'v1', next_cursor: 'next-v1', truncated: true }); await load;
  expect(held()).toMatchObject({ watch: 'stale', invalidated: true, nextCursor: null });
  expect(held()?.entries[0]?.name).toBe('page-one.ts');
  expect(calls.list).toHaveBeenCalledTimes(1);
  calls.list.mockResolvedValueOnce({ ...listing('fresh.ts'), revision: 'v2', next_cursor: 'next-v2', truncated: true });
  await directoryWriter.retry(door, '');
  expect(held()).toMatchObject({ watch: 'live', invalidated: false, revision: 'v2', nextCursor: 'next-v2' });
  expect(subscriptions).toHaveLength(1); release();
});

it('adopts changed continuation metadata even when the visible names have not changed', async () => {
  calls.list.mockResolvedValueOnce({ ...listing('same.ts'), revision: 'v1', next_cursor: 'one', truncated: true });
  await directoryWriter.load(door, '');
  calls.list.mockResolvedValueOnce({ ...listing('same.ts'), revision: 'v2', next_cursor: 'two', truncated: true });
  await directoryWriter.load(door, '');
  expect(held()).toMatchObject({ revision: 'v2', nextCursor: 'two' });
});

it('does not share an obsolete pending read with an explicit retry after invalidation', async () => {
  const release = watchDirectory(door, ''); subscriptions[0]?.onFrame(update('old.ts'));
  const old = pending<DirListing>(); calls.list.mockReturnValueOnce(old.promise);
  const loading = directoryWriter.load(door, '');
  subscriptions[0]?.onFrame({ op: 'event', id: 1, feed: 'directory', data: { invalidated: true } });
  calls.list.mockResolvedValueOnce(listing('fresh.ts'));
  await directoryWriter.retry(door, '');
  expect(calls.list).toHaveBeenCalledTimes(2);
  old.resolve(listing('obsolete.ts')); await loading;
  expect(held()?.entries[0]?.name).toBe('fresh.ts'); release();
});

it('preserves native page order and uses Unicode scalar ordering for complete live deltas', async () => {
  const names = ['Z', 'z', 'ä', '\uE000', '\u{10000}'];
  calls.list.mockResolvedValueOnce({ ...listing('unused'), entries: names.map(entry) });
  await directoryWriter.load(door, '');
  expect(held()?.entries.map((item) => item.name)).toEqual(names);
  expect(sortEntries([...names].reverse().map(entry)).map((item) => item.name)).toEqual(names);
});
