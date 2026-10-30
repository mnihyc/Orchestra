/** Deterministic store dependencies; no transport, timer, real account or native process. */
import { vi } from 'vitest';
import type { Door } from '@/api/workspace';
import type { DirData, DirEntry, DirListing, FileListSource } from '@/generated/api-types';
import type { FeedFrame } from '@/realtime/socket';
import { DirectoryWindows } from '@/realtime/stores/directoryWindows';
import type { DirectoryLease, DirectoryState } from '@/realtime/stores/directoryTypes';

export const door: Door = { environmentId: 'environment', threadId: 'thread' };
export const STAMP = '2026-01-01T00:00:00.000Z';
export function entry(name: string, parent = '', type: DirEntry['type'] = 'file'): DirEntry {
  return { name, path: parent === '' ? name : `${parent}/${name}`, type, size: 1, mtime: STAMP,
    cached: false, mode: 420, link_target: null };
}
export function page(names: readonly string[], options: Partial<DirListing> = {}): DirListing {
  return { path: '', entries: names.map((name) => entry(name, options.path)), mount: null, source: 'live',
    as_of: STAMP, observed_from: STAMP, truncated: false, revision: 'revision', next_cursor: null, expires_at: null, ...options };
}
export function pending<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
export interface ReadOptions { source: FileListSource; cursor?: string; limit: number; signal: AbortSignal }
export function rig() {
  let clock = Date.parse(STAMP), reader = 1;
  let published: ReadonlyMap<string, DirectoryState> = new Map();
  const subscriptions: { feed: string; onFrame: (frame: FeedFrame) => void; unsubscribe: ReturnType<typeof vi.fn> }[] = [];
  const fetch = vi.fn((_door: Door, path: string, _options: ReadOptions) => Promise.resolve(page([], { path })));
  const store = new DirectoryWindows({ reader: () => reader, now: () => clock, fetch,
    publish: (dirs) => { published = dirs; },
    subscribe: (feed, options) => {
      const subscription = { feed, onFrame: options.onFrame, unsubscribe: vi.fn() };
      subscriptions.push(subscription); return subscription;
    },
  });
  return { store, fetch, subscriptions,
    advance: (ms: number) => { clock += ms; },
    reader: () => { reader += 1; store.clear(); },
    published: () => published,
    open: (path = '', pane = 'one', binding = 'binding', parent?: DirectoryLease): DirectoryLease => {
      const result = store.open(door, path, { pane, binding, ...(parent === undefined ? {} : { parent }) });
      if (!result.ok) throw new Error(`Unexpected admission refusal: ${result.reason}`);
      return result.lease;
    },
    event: (data: Partial<DirData> & { offline?: boolean }, index = 0) => {
      const subscription = subscriptions[index];
      if (subscription === undefined) throw new Error('No native subscription');
      subscription.onFrame({ op: 'event', id: index + 1, feed: subscription.feed, data });
    },
  };
}
