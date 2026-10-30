/** Native machine/job snapshots update only visible cached views; reconnects use the held cursor and never HTTP. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { describe, expect, it, vi } from 'vitest';
import { applyQueryFrame, installQueryFeeds, queryFeed, waitForJob } from '@/realtime/queryFeeds';
import type { FeedFrame, FeedListener, RealtimeSocket } from '@/realtime/socket';
import type { Job } from '@/generated/api-types';
import { deliverLiveValue, liveValue } from '@/api/liveValue';
import { feedSequences } from '@/api/cache';
import { detailEnvironment } from '../component/resource-detail-fixtures';

function wire() {
  const listeners = new Map<string, FeedListener>();
  const unsubscribe = vi.fn();
  const subscribe = vi.fn((feed: string, listener: FeedListener) => { listeners.set(feed, listener); return { unsubscribe }; });
  return { listeners, subscribe, unsubscribe, socket: { subscribe } as unknown as RealtimeSocket };
}
const event = (data: Record<string, unknown>): FeedFrame => ({ op: 'event', id: 1, feed: 'env:e1:procs', data });
const sync = (seq: number, data: Record<string, unknown>): FeedFrame => ({ op: 'sync', id: 1, feed: 'env:e1:procs', prev: seq - 1, seq, data });

describe('visible subscriptions', () => {
  it('shares one canonical environment feed across two panes and rejects late HTTP and older native rows', async () => {
    const cache = new QueryClient(), live = wire(), key = ['environment', detailEnvironment.id];
    let finish!: (value: typeof detailEnvironment) => void;
    let aborted = false;
    const read = vi.fn(({ signal }: { signal: AbortSignal }) => {
      signal.addEventListener('abort', () => { aborted = true; });
      return new Promise<typeof detailEnvironment>((resolve) => { finish = resolve; });
    });
    const one = new QueryObserver(cache, { queryKey: key, queryFn: read });
    const two = new QueryObserver(cache, { queryKey: key, queryFn: read });
    const offOne = one.subscribe(() => undefined), offTwo = two.subscribe(() => undefined);
    const stop = installQueryFeeds(live.socket, cache), name = `env:${detailEnvironment.id}`;
    expect(live.subscribe).toHaveBeenCalledTimes(1); expect(read).toHaveBeenCalledTimes(1);
    const current = { ...detailEnvironment, generation: 2, revision: 2, seq: 2 };
    live.listeners.get(name)?.onFrame(sync(2, { environment: current, jobs: [] }));
    expect(aborted).toBe(true);
    finish(detailEnvironment); await Promise.resolve(); await Promise.resolve();
    live.listeners.get(name)?.onFrame(sync(1, { environment: detailEnvironment }));
    expect(cache.getQueryData(key)).toEqual(current);
    expect(live.listeners.get(name)?.since()).toBe(2);
    offOne(); await Promise.resolve(); expect(live.unsubscribe).not.toHaveBeenCalled();
    offTwo(); await Promise.resolve(); expect(live.unsubscribe).toHaveBeenCalledTimes(1);
    expect(read).toHaveBeenCalledTimes(1);
    stop(); cache.clear(); feedSequences.delete(name);
  });
  it('retires a native authority projection immediately and reloads its environment cut before resubscribing', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }), live = wire();
    const key = ['environment', detailEnvironment.id], name = `env:${detailEnvironment.id}`;
    const reads: { signal: AbortSignal; resolve: (value: typeof detailEnvironment) => void }[] = [];
    const read = vi.fn(({ signal }: { signal: AbortSignal }) => new Promise<typeof detailEnvironment>((resolve) => reads.push({ signal, resolve })));
    cache.setQueryData(key, detailEnvironment); feedSequences.set(name, 1);
    const observer = new QueryObserver(cache, { queryKey: key, queryFn: read });
    const off = observer.subscribe(() => undefined), stop = installQueryFeeds(live.socket, cache);
    const former = reads[0]!;
    live.listeners.get(name)?.onFrame({ op: 'resync', id: 1, feed: name, reload: true });
    expect(former.signal.aborted).toBe(true);
    expect(cache.getQueryData(key)).toBeUndefined();
    expect(reads).toHaveLength(2);
    expect(live.subscribe).toHaveBeenCalledTimes(1);
    former.resolve(detailEnvironment);
    for (let turn = 0; turn < 8; turn += 1) await Promise.resolve();
    expect(cache.getQueryData(key)).toBeUndefined();
    const current = { ...detailEnvironment, generation: 2, revision: 2, seq: 2 };
    feedSequences.set(name, 2); reads[1]!.resolve(current);
    for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    expect(cache.getQueryData(key)).toEqual(current);
    expect(live.subscribe).toHaveBeenCalledTimes(2);
    expect(live.listeners.get(name)?.since()).toBe(2);
    off(); stop(); cache.clear(); feedSequences.delete(name);
  });

  it('rejoins a complete native snapshot at the same sequence without HTTP and removes hidden former projections', () => {
    const cache = new QueryClient(), live = wire();
    const key = ['processes', 'e1'], hidden = ['processes', 'e1', 'hidden'], other = ['processes', 'e2'];
    const former = { as_of: 'before', processes: [{ pid: 12 }] };
    const current = { as_of: 'now', processes: [] };
    const read = vi.fn(() => Promise.resolve(former));
    for (const one of [key, hidden, other]) cache.setQueryData(one, former);
    const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
    const off = observer.subscribe(() => undefined), stop = installQueryFeeds(live.socket, cache);
    live.listeners.get('env:e1:procs')?.onFrame(sync(7, former));
    live.listeners.get('env:e1:procs')?.onFrame({ op: 'resync', id: 1, feed: 'env:e1:procs', reload: true });
    expect(cache.getQueryData(key)).toBeUndefined();
    expect(cache.getQueryData(hidden)).toBeUndefined();
    expect(cache.getQueryData(other)).toEqual(former);
    expect(live.listeners.get('env:e1:procs')?.since()).toBeNull();
    live.listeners.get('env:e1:procs')?.onFrame(sync(7, current));
    expect(cache.getQueryData(key)).toEqual(current);
    expect(read).not.toHaveBeenCalled();
    off(); stop(); cache.clear();
  });

  it('waits for an already pending HTTP baseline on reload acknowledgement and never starts a duplicate read', async () => {
    const cache = new QueryClient(), live = wire(), key = ['environment', detailEnvironment.id], name = `env:${detailEnvironment.id}`;
    let finish!: (value: typeof detailEnvironment) => void;
    const read = vi.fn(() => new Promise<typeof detailEnvironment>((resolve) => { finish = resolve; }));
    const observer = new QueryObserver(cache, { queryKey: key, queryFn: read });
    const off = observer.subscribe(() => undefined), stop = installQueryFeeds(live.socket, cache);
    live.listeners.get(name)?.onFrame({ op: 'subscribed', id: 1, feed: name, seq: 1, mode: 'reload' });
    expect(read).toHaveBeenCalledTimes(1);
    feedSequences.set(name, 1); finish(detailEnvironment);
    for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    expect(live.subscribe).toHaveBeenCalledTimes(2);
    expect(live.listeners.get(name)?.since()).toBe(1);
    expect(read).toHaveBeenCalledTimes(1);
    off(); stop(); cache.clear(); feedSequences.delete(name);
  });

  it('keeps a refused authority baseline empty without a repair loop until the visible owner retries', async () => {
    const cache = new QueryClient({ defaultOptions: { queries: { retry: false } } }), live = wire();
    const key = ['environment', detailEnvironment.id], name = `env:${detailEnvironment.id}`;
    cache.setQueryData(key, detailEnvironment); feedSequences.set(name, 1);
    const read = vi.fn().mockRejectedValueOnce(new Error('reader refused')).mockResolvedValue(detailEnvironment);
    const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
    const off = observer.subscribe(() => undefined), stop = installQueryFeeds(live.socket, cache);
    live.listeners.get(name)?.onFrame({ op: 'resync', id: 1, feed: name, reload: true });
    for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    expect(cache.getQueryData(key)).toBeUndefined();
    expect(cache.getQueryState(key)?.status).toBe('error');
    expect(read).toHaveBeenCalledTimes(1);
    expect(live.subscribe).toHaveBeenCalledTimes(1);
    await observer.refetch();
    for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    expect(cache.getQueryData(key)).toEqual(detailEnvironment);
    expect(read).toHaveBeenCalledTimes(2);
    off(); stop(); cache.clear(); feedSequences.delete(name);
  });

  it('refreshes the browser capability baseline before partial native lists can reuse an old privileged view', async () => {
    const cache = new QueryClient(), live = wire(), key = ['environment-browsers', 'e1'];
    const previous = { support: { programs: ['old-private-program'] }, browsers: [], profiles: [{ id: 'private' }],
      presets: [], profiles_dir: '/private', off: null, limit: { open: 0, max: 3 } };
    const current = { ...previous, support: { programs: [] }, profiles: [], profiles_dir: null, limit: { open: 0, max: 0 } };
    cache.setQueryData(key, previous);
    const read = vi.fn(() => Promise.resolve(current));
    const observer = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
    const off = observer.subscribe(() => undefined), stop = installQueryFeeds(live.socket, cache);
    live.listeners.get('env:e1:browsers')?.onFrame({ op: 'resync', id: 1, feed: 'env:e1:browsers', reload: true });
    expect(cache.getQueryData(key)).toBeUndefined();
    for (let turn = 0; turn < 12; turn += 1) await Promise.resolve();
    expect(read).toHaveBeenCalledTimes(1);
    live.listeners.get('env:e1:browsers')?.onFrame(event({ browsers: [], profiles: [] }));
    expect(cache.getQueryData(key)).toEqual(current);
    off(); stop(); cache.clear();
  });

  it('answers the initial read over the native subscription and releases a closed view’s pending read', async () => {
    const cache=new QueryClient();const live=wire();const key=['processes','e1'];
    const initial=liveValue(key,new AbortController().signal);
    const observer=new QueryObserver(cache,{queryKey:key,queryFn:()=>initial});
    const off=observer.subscribe(()=>undefined);const stop=installQueryFeeds(live.socket,cache);
    const value={as_of:'now',processes:[{pid:12}]};
    live.listeners.get('env:e1:procs')?.onFrame(event(value));
    await expect(initial).resolves.toEqual(value);
    const abort=new AbortController();const pending=liveValue(['job','cancelled-view'],abort.signal);
    const refused=expect(pending).rejects.toMatchObject({name:'AbortError'});abort.abort();await refused;
    deliverLiveValue(['job','cancelled-view'],{id:'late'});
    off();stop();cache.clear();
  });

  it('keeps health on its own lightweight scoped feed and never subscribes to request rows', () => {
    const cache=new QueryClient();const key=['usage','health','p1'];
    expect(queryFeed(key)?.feed).toBe('usage:provider:p1:health');
    const health={since:'now',recent:{requests:12},baseline:{requests:32},errors:false,first_output:true};
    applyQueryFrame(cache,key,event({health}));
    expect(cache.getQueryData(key)).toEqual(health);
    expect(queryFeed(['workspace-git','e1','t1'])?.feed).toBe('thread:t1:git');
    cache.clear();
  });

  it('shares a source, stops it as soon as hidden, and resumes from its last sequence', async () => {
    const cache = new QueryClient(); const live = wire();
    const key = ['processes', 'e1', 't1'];
    const read = vi.fn(() => Promise.resolve({ as_of: 'before', processes: [] }));
    cache.setQueryData(key, { as_of: 'before', processes: [] });
    const one = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
    const two = new QueryObserver(cache, { queryKey: key, queryFn: read, staleTime: Infinity });
    const offOne = one.subscribe(() => undefined); const offTwo = two.subscribe(() => undefined);
    const stop = installQueryFeeds(live.socket, cache);
    expect(live.subscribe).toHaveBeenCalledTimes(1);
    live.listeners.get('env:e1:procs')?.onFrame(sync(7, { as_of: 'now', processes: [{ pid: 12 }] }));
    expect(cache.getQueryData(key)).toEqual({ as_of: 'now', processes: [{ pid: 12 }] });
    expect(live.listeners.get('env:e1:procs')?.since()).toBe(7);
    offOne(); offTwo(); await Promise.resolve();
    expect(live.unsubscribe).toHaveBeenCalledTimes(1);
    const offAgain = one.subscribe(() => undefined); await Promise.resolve();
    expect(live.subscribe).toHaveBeenCalledTimes(2);
    expect(live.listeners.get('env:e1:procs')?.since()).toBe(7);
    expect(read).not.toHaveBeenCalled();
    offAgain(); stop(); cache.clear();
  });

  it('subscribes no query whose observer is disabled', () => {
    const cache = new QueryClient(); const live = wire();
    const observer = new QueryObserver(cache, { queryKey: ['processes', 'e1', null], queryFn: () => Promise.resolve([]), enabled: false });
    const off = observer.subscribe(() => undefined); const stop = installQueryFeeds(live.socket, cache);
    expect(live.subscribe).not.toHaveBeenCalled();
    off(); stop(); cache.clear();
  });

  it('decodes the native terminal pages and appends resource samples without dropping history', () => {
    const cache = new QueryClient();
    const open = ['terminals', 'e1', 't1']; const closed = [...open, 'closed'];
    const terminal = { id: 'shell-1' };
    const frame = event({ open: { items: [terminal], next_cursor: null }, closed: { items: [], next_cursor: null } });
    applyQueryFrame(cache, open, frame); applyQueryFrame(cache, closed, frame);
    expect(cache.getQueryData(open)).toEqual([terminal]); expect(cache.getQueryData(closed)).toEqual([]);
    const key = ['resources', 'e1', 't1', 60];
    const first = { at: '2026-10-15T15:30:00Z', cpu_percent: 10 };
    const next = { at: '2026-10-15T15:30:05Z', cpu_percent: 20 };
    cache.setQueryData(key, [first]);
    applyQueryFrame(cache, key, event(next)); applyQueryFrame(cache, key, event(next));
    expect(cache.getQueryData(key)).toEqual([first, next]);
    expect(queryFeed(['workspace-git', 'e1'])?.feed).toBe('env:e1:git:');
    cache.clear();
  });

  it('waits for a fork’s own job result without a timer or another request', async () => {
    const cache = new QueryClient(); const live = wire();
    const job = { id: 'j1', state: 'running', progress: { percent: 1, phase: 'copy' }, result: null } as unknown as Job;
    const stop = installQueryFeeds(live.socket, cache);
    const waiting = waitForJob(cache, job);
    await Promise.resolve();
    const ended = { ...job, state: 'succeeded', progress: { percent: 100, phase: 'done' } };
    live.listeners.get('job:j1')?.onFrame(sync(8, { job: ended }));
    expect(await waiting).toEqual(ended);
    expect(live.unsubscribe).toHaveBeenCalledTimes(1);
    expect(cache.getQueryData(['job', 'j1'])).toEqual(ended);
    stop(); cache.clear();
  });
});
