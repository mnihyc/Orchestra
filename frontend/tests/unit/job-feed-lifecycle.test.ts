/** A pending act and visible job pane share the existing native query owner. */
import { QueryClient, QueryObserver } from '@tanstack/react-query';
import { afterEach, expect, it, vi } from 'vitest';
import { jobQuery } from '@/api/forks';
import { installQueryFeeds, waitForJob } from '@/realtime/queryFeeds';
import type { FeedFrame, FeedListener, RealtimeSocket } from '@/realtime/socket';
import type { Job } from '@/generated/api-types';

const job = { id: 'authority-job', state: 'running', progress: { percent: 1, phase: 'copy' }, result: null } as unknown as Job;
const ended: Job = { ...job, state: 'succeeded', progress: { ...job.progress, percent: 100, phase: 'done' } };
const feed = `job:${job.id}`;
const cleanups: (() => void)[] = [];
const settle = async () => { for (let turn = 0; turn < 12; turn += 1) await Promise.resolve(); };
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); });
function rig() {
  const cache = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity } } });
  const listeners: (FeedListener & { unsubscribe: ReturnType<typeof vi.fn> })[] = [];
  const subscribe = vi.fn((_feed: string, listener: FeedListener) => {
    const lease = { ...listener, unsubscribe: vi.fn() }; listeners.push(lease); return lease;
  });
  const socket = { subscribe } as unknown as RealtimeSocket;
  const stop = installQueryFeeds(socket, cache);
  cleanups.push(() => { stop(); cache.clear(); });
  const event = (value: Job, at = listeners.length - 1) => listeners[at]?.onFrame({ op: 'event', id: at + 1, feed, data: { job: value } });
  const send = (value: FeedFrame, at = listeners.length - 1) => listeners[at]?.onFrame(value);
  const wait = () => waitForJob(cache, job);
  return { cache, listeners, subscribe, event, send, wait };
}

it('uses one native subscription for both the pending act and a visible job pane, releasing each independently', async () => {
  const test = rig();
  const pane = new QueryObserver(test.cache, { ...jobQuery(job.id), initialData: job, staleTime: Infinity });
  const leave = pane.subscribe(() => undefined);
  const waiting = test.wait(); await settle();
  expect(test.subscribe).toHaveBeenCalledTimes(1);
  test.event(ended); await expect(waiting).resolves.toEqual(ended); await settle();
  expect(test.cache.getQueryData(['job', job.id])).toEqual(ended);
  expect(test.listeners[0]?.unsubscribe).not.toHaveBeenCalled();
  leave(); await settle();
  expect(test.listeners[0]?.unsubscribe).toHaveBeenCalledTimes(1);
  test.event(job, 0);
  expect(test.cache.getQueryData(['job', job.id])).toEqual(ended);
});

it('retires authority content and old callbacks, rejoins the native snapshot, and completes without a second writer', async () => {
  const test = rig(), waiting = test.wait(); await settle();
  test.event(job);
  test.send({ op: 'resync', id: 1, feed, reload: true });
  expect(test.cache.getQueryData(['job', job.id])).toBeUndefined();
  expect(test.subscribe).toHaveBeenCalledTimes(2);
  expect(test.listeners[0]?.unsubscribe).toHaveBeenCalledTimes(1);
  expect(test.listeners[1]?.since()).toBeNull();
  test.event(ended, 0);
  expect(test.cache.getQueryData(['job', job.id])).toBeUndefined();
  test.event(ended, 1); await expect(waiting).resolves.toEqual(ended); await settle();
  expect(test.listeners[1]?.unsubscribe).toHaveBeenCalledTimes(1);
  test.event(job, 1);
  expect(test.cache.getQueryData(['job', job.id])).toEqual(ended);
});

it('keeps capacity waiting demand until the socket promotes that same subscription', async () => {
  const test = rig();
  let outcome = 'pending';
  const waiting = test.wait().then((value) => { outcome = 'resolved'; return value; }, () => { outcome = 'rejected'; return null; });
  await settle();
  test.send({ op: 'error', id: 1, code: 'too_many_subscriptions', detail: 'Waiting for capacity' });
  await settle();
  expect(outcome).toBe('pending');
  expect(test.subscribe).toHaveBeenCalledTimes(1);
  expect(test.listeners[0]?.unsubscribe).not.toHaveBeenCalled();
  test.send({ op: 'subscribed', id: 1, feed, seq: null, mode: 'ephemeral' });
  test.event(ended); await expect(waiting).resolves.toEqual(ended); await settle();
  expect(test.listeners[0]?.unsubscribe).toHaveBeenCalledTimes(1);
});

it.each(['revoked', 'deleted'] as const)('retires a %s job projection and rejects the act without retaining private cached result', async (reason) => {
  const test = rig();
  const waiting = test.wait().then(() => null, (error: unknown) => error); await settle();
  const privateJob = { ...job, result: { private_detail: 'former authority' } } as unknown as Job;
  test.event(privateJob);
  test.send({ op: 'closed', id: 1, feed, reason });
  expect(test.cache.getQueryData(['job', job.id])).toBeUndefined();
  expect(await waiting).toBeInstanceOf(Error); await settle();
  expect(test.listeners[0]?.unsubscribe).toHaveBeenCalledTimes(1);
  test.event(privateJob, 0);
  expect(test.cache.getQueryData(['job', job.id])).toBeUndefined();
});
