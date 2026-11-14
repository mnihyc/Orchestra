/** A wait that only the live connection can end reads again by itself: often while the connection is down, seldom while it is up, never when the wait is over. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { deliverLiveValue, liveValue, LIVE_VALUE_PATIENCE_MS } from '@/api/liveValue';
import { READ_AGAIN_DOWN_MS, READ_AGAIN_UP_MS, readAgain, readAgainAfter, readAgainFollows } from '@/api/readAgain';

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { vi.useRealTimers(); readAgainFollows(() => true); });

const holding = <Data>(data: Data | undefined) => ({ state: { data } });

it('names two intervals, often for a connection that is down and seldom for one that is up', () => {
  expect(READ_AGAIN_DOWN_MS).toBeLessThan(READ_AGAIN_UP_MS);
  readAgainFollows(() => true); expect(readAgainAfter()).toBe(READ_AGAIN_UP_MS);
  readAgainFollows(() => false); expect(readAgainAfter()).toBe(READ_AGAIN_DOWN_MS);
});

it('reads again only while the wait is on', () => {
  const interval = readAgain<{ done: boolean }>((data) => data?.done !== true);
  readAgainFollows(() => false);
  expect(interval(holding(undefined))).toBe(READ_AGAIN_DOWN_MS);
  expect(interval(holding({ done: false }))).toBe(READ_AGAIN_DOWN_MS);
  expect(interval(holding({ done: true }))).toBe(false);
  readAgainFollows(() => true);
  expect(interval(holding({ done: false }))).toBe(READ_AGAIN_UP_MS);
  expect(interval(holding({ done: true }))).toBe(false);
});

it('can leave a query alone while the connection is up, for an answer that is the next live snapshot', () => {
  const interval = readAgain<number>((data) => data === undefined, false);
  readAgainFollows(() => true); expect(interval(holding(undefined))).toBe(false);
  readAgainFollows(() => false); expect(interval(holding(undefined))).toBe(READ_AGAIN_DOWN_MS);
});

it('reads over HTTP at once when the connection is not up, and the live snapshot is not waited for', async () => {
  readAgainFollows(() => false);
  const read = vi.fn(() => Promise.resolve('read'));
  await expect(liveValue(['job', 'down'], new AbortController().signal, read)).resolves.toBe('read');
  expect(read).toHaveBeenCalledTimes(1);
});

it('waits for the live snapshot while the connection is up, and reads when it has not come in time', async () => {
  readAgainFollows(() => true);
  const read = vi.fn(() => Promise.resolve('read'));
  const first = liveValue(['job', 'snapshot'], new AbortController().signal, read);
  deliverLiveValue(['job', 'snapshot'], 'live');
  await expect(first).resolves.toBe('live');
  await vi.advanceTimersByTimeAsync(LIVE_VALUE_PATIENCE_MS);
  expect(read).not.toHaveBeenCalled();
  const late = liveValue(['job', 'late'], new AbortController().signal, read);
  await vi.advanceTimersByTimeAsync(LIVE_VALUE_PATIENCE_MS);
  await expect(late).resolves.toBe('read');
  expect(read).toHaveBeenCalledTimes(1);
});

it('keeps its old behaviour without a read, and an aborted wait is rejected', async () => {
  readAgainFollows(() => false);
  const controller = new AbortController();
  const waiting = liveValue(['job', 'plain'], controller.signal);
  controller.abort();
  await expect(waiting).rejects.toMatchObject({ name: 'AbortError' });
});
