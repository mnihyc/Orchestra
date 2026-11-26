// @vitest-environment node
/** A read that was cancelled or overtaken is no failure of a feed, however often the reader changes the demand. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { NetworkError, READ_RETRY_PAUSES_MS } from '@/lib/requests';
import { supersededRead } from '@/api/reads';
import type { RealtimeSocket } from '@/realtime/socket';
import { type DurableFeed, followFeed, isCancellation, loadWithRetries, type Loaded } from '@/realtime/sync';
import { client } from '@/api/client';
import { readAttemptFacts, requestReader } from '@/lib/readAttempts';

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
  const resetting = vi.fn();
  const held: string[] = [];
  const feed: DurableFeed<Data> = { feed: 'thread:cancelled-read', load, failed, resetting,
    read: (rows) => ({ words: Array.isArray(rows.words) ? rows.words.filter((word: unknown): word is string => typeof word === 'string') : [] }),
    reset: (data) => { held.splice(0, held.length, ...data.words); }, apply: (data) => { held.splice(0, held.length, ...data.words); } };
  return { follower: followFeed(socket, feed), failed, held, resetting };
}

it('knows a cancelled read from a failed one', () => {
  expect(isCancellation(cancelled())).toBe(true);
  expect(isCancellation(new DOMException('Timed out', 'TimeoutError'))).toBe(false);
  expect(isCancellation(new Error('The server refused'))).toBe(false);
  expect(isCancellation(supersededRead())).toBe(true);
});

it('ends an admitted native retry when its view leaves without making another hidden read', async () => {
  const error = new NetworkError('failed'), controller = new AbortController();
  const reader = client.session.getState(), identity = requestReader(reader.userId, reader.readerVersion);
  const load = vi.fn<() => Promise<string>>().mockRejectedValue(error);
  let terminal: unknown;
  const done = loadWithRetries(load, controller.signal).catch((failure: unknown) => { terminal = failure; });
  try {
    const at = Date.now(); await vi.advanceTimersByTimeAsync(0);
    expect(readAttemptFacts(error, identity)).toMatchObject({ read_attempt: 1, phase: 'waiting', scheduled_retry_at: at + READ_RETRY_PAUSES_MS[0] });
    controller.abort(); await vi.advanceTimersByTimeAsync(0); await done;
    expect(terminal).toMatchObject({ name: 'AbortError' });
    await vi.advanceTimersByTimeAsync(10_000); expect(load).toHaveBeenCalledOnce(); expect(readAttemptFacts(error, identity)).toBeNull();
  } finally { controller.abort(); await vi.advanceTimersByTimeAsync(10_000); await done; }
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

it('keeps the chat live through repeated superseded reads and loads the current demand without a manual refresh', async () => {
  const cancellations = READ_RETRY_PAUSES_MS.length + 2;
  let attempts = 0;
  const load = vi.fn<DurableFeed<Data>['load']>().mockImplementation(() => {
    if (attempts++ < cancellations) return Promise.reject(cancelled());
    return Promise.resolve({ seq: 7, complete: true, data: { words: ['current demand'] } });
  });
  const { follower, failed, held } = follow(load);
  await vi.advanceTimersByTimeAsync(0);
  for (let attempt = 0; attempt < cancellations; attempt += 1) {
    expect(failed).not.toHaveBeenCalled();
    const pause = READ_RETRY_PAUSES_MS[Math.min(attempt, READ_RETRY_PAUSES_MS.length - 1)];
    if (pause === undefined) throw new Error('Cancelled reads use the shared read backoff');
    await vi.advanceTimersByTimeAsync(pause);
  }
  expect(load).toHaveBeenCalledTimes(cancellations + 1);
  expect(held).toEqual(['current demand']);
  expect(follower.lastSeq()).toBe(7);
  expect(failed).not.toHaveBeenCalled();
  follower.stop();
});

it('retires a cancelled read when the chat leaves and makes no delayed read afterward', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>().mockRejectedValue(cancelled());
  const { follower, failed } = follow(load);
  await vi.advanceTimersByTimeAsync(0);
  follower.stop();
  await vi.runAllTimersAsync();
  expect(load).toHaveBeenCalledOnce();
  expect(failed).not.toHaveBeenCalled();
});

it('keeps the admitted window and cursor when a refresh is superseded rather than reopening the chat’s tail', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>()
    .mockResolvedValueOnce({ seq: 7, complete: true, data: { words: ['admitted window'] } })
    .mockRejectedValueOnce(cancelled())
    .mockResolvedValueOnce({ seq: 8, complete: true, data: { words: ['current window'] } });
  const { follower, held, failed, resetting } = follow(load);
  await vi.advanceTimersByTimeAsync(0);
  follower.refresh();
  await vi.advanceTimersByTimeAsync(0);
  expect(follower.lastSeq()).toBe(7);
  expect(held).toEqual(['admitted window']);
  expect(resetting).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
  expect(load.mock.calls.map(([since]) => since)).toEqual([null, 7, 7]);
  expect(held).toEqual(['current window']);
  expect(failed).not.toHaveBeenCalled();
  follower.stop();
});

it('reports a read that really failed at once and does not ask again by itself', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>().mockRejectedValue(new Error('The server refused'));
  const { failed } = follow(load);
  await vi.advanceTimersByTimeAsync(10_000);
  expect(failed).toHaveBeenCalledTimes(1);
  expect(load).toHaveBeenCalledTimes(1);
});

it('recovers the current durable window silently when a shared read is superseded', async () => {
  const load = vi.fn<DurableFeed<Data>['load']>()
    .mockResolvedValueOnce({ seq: 7, complete: true, data: { words: ['admitted window'] } })
    .mockRejectedValueOnce(supersededRead())
    .mockResolvedValueOnce({ seq: 8, complete: true, data: { words: ['newer window'] } });
  const { follower, held, failed, resetting } = follow(load);
  await vi.advanceTimersByTimeAsync(0);
  follower.refresh(); await vi.advanceTimersByTimeAsync(0);
  expect(failed).not.toHaveBeenCalled(); expect(held).toEqual(['admitted window']);
  expect(follower.lastSeq()).toBe(7); expect(resetting).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(READ_RETRY_PAUSES_MS[0]);
  expect(load.mock.calls.map(([since]) => since)).toEqual([null, 7, 7]);
  expect(held).toEqual(['newer window']); expect(follower.lastSeq()).toBe(8);
  expect(failed).not.toHaveBeenCalled(); follower.stop();
});


it('joins simultaneous cursor refreshes into one catch-up and still reads a later demand', async () => {
  let finish: ((value: Loaded<Data>) => void) | undefined;
  const load = vi.fn<DurableFeed<Data>['load']>()
    .mockResolvedValueOnce({ seq: 7, complete: true, data: { words: ['admitted window'] } })
    .mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }))
    .mockResolvedValue({ seq: 9, complete: true, data: { words: ['later demand'] } });
  const { follower, held } = follow(load);
  try {
    await vi.advanceTimersByTimeAsync(0);
    follower.refresh(); follower.refresh();
    await vi.advanceTimersByTimeAsync(0);
    expect(load.mock.calls.map(([since]) => since)).toEqual([null, 7]);
    finish?.({ seq: 8, complete: true, data: { words: ['caught up'] } });
    await vi.advanceTimersByTimeAsync(0);
    expect(load.mock.calls.map(([since]) => since)).toEqual([null, 7]);
    expect(held).toEqual(['caught up']);
    follower.refresh(); await vi.advanceTimersByTimeAsync(0);
    expect(load.mock.calls.map(([since]) => since)).toEqual([null, 7, 8]);
    expect(held).toEqual(['later demand']);
  } finally { follower.stop(); }
});


it('keeps a new refresh received during transport because it can describe a later write', async () => {
  let finish: ((value: Loaded<Data>) => void) | undefined;
  const load = vi.fn<DurableFeed<Data>['load']>()
    .mockResolvedValueOnce({ seq: 7, complete: true, data: { words: ['admitted window'] } })
    .mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }))
    .mockResolvedValue({ seq: 9, complete: true, data: { words: ['newly committed write'] } });
  const { follower, held } = follow(load);
  try {
    await vi.advanceTimersByTimeAsync(0);
    follower.refresh(); await vi.advanceTimersByTimeAsync(0);
    follower.refresh();
    finish?.({ seq: 8, complete: true, data: { words: ['earlier snapshot'] } });
    await vi.advanceTimersByTimeAsync(0);
    expect(load.mock.calls.map(([since]) => since)).toEqual([null, 7, 8]);
    expect(held).toEqual(['newly committed write']);
  } finally { follower.stop(); }
});
