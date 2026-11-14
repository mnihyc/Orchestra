// @vitest-environment node
/** A read that was cancelled or overtaken is no failure of a feed: it is asked for again, and a feed only says it failed when asking again keeps being cancelled or the read really failed. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { READ_RETRY_PAUSES_MS } from '@/lib/requests';
import type { RealtimeSocket } from '@/realtime/socket';
import { type DurableFeed, followFeed, isCancellation, type Loaded } from '@/realtime/sync';

interface Data { words: string[] }
const socket = { subscribe: () => ({ unsubscribe: () => undefined }) } as unknown as RealtimeSocket;
const cancelled = () => new DOMException('The thread read selection changed', 'AbortError');

beforeEach(() => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function follow(load: DurableFeed<Data>['load']) {
  const failed = vi.fn();
  const held: string[] = [];
  const feed: DurableFeed<Data> = { feed: 'thread:cancelled-read', load, failed,
    read: (rows) => ({ words: Array.isArray(rows.words) ? rows.words.filter((word: unknown): word is string => typeof word === 'string') : [] }),
    reset: (data) => { held.splice(0, held.length, ...data.words); }, apply: (data) => { held.splice(0, held.length, ...data.words); } };
  return { follower: followFeed(socket, feed), failed, held };
}

it('knows a cancelled read from a failed one', () => {
  expect(isCancellation(cancelled())).toBe(true);
  expect(isCancellation(new DOMException('Timed out', 'TimeoutError'))).toBe(false);
  expect(isCancellation(new Error('The server refused'))).toBe(false);
});

it('asks again after a cancelled read and does not report it', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>()
    .mockRejectedValueOnce(cancelled())
    .mockResolvedValue({ seq: 1, complete: true, data: { words: ['whole'] } } satisfies Loaded<Data>);
  const { follower, failed, held } = follow(load);
  await vi.advanceTimersByTimeAsync(0);
  expect(failed).not.toHaveBeenCalled();
  expect(held).toEqual([]);
  await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
  expect(load).toHaveBeenCalledTimes(2);
  expect(held).toEqual(['whole']);
  expect(failed).not.toHaveBeenCalled();
  expect(follower.lastSeq()).toBe(1);
});

it('says that the feed failed only when every attempt is cancelled', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>().mockImplementation(() => Promise.reject(cancelled()));
  const { failed } = follow(load);
  await vi.advanceTimersByTimeAsync(0);
  for (const pause of READ_RETRY_PAUSES_MS) {
    expect(failed).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(pause);
  }
  expect(load).toHaveBeenCalledTimes(READ_RETRY_PAUSES_MS.length + 1);
  expect(failed).toHaveBeenCalledTimes(1);
  expect(isCancellation(failed.mock.calls[0]?.[0])).toBe(true);
});

it('reports a read that really failed at once and does not ask again by itself', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>().mockRejectedValue(new Error('The server refused'));
  const { failed } = follow(load);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(failed).toHaveBeenCalledTimes(1);
  expect(load).toHaveBeenCalledTimes(1);
});
