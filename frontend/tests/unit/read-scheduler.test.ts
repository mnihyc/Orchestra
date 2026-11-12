/** Admission/aggregate ownership controls, independent of HTTP timing and screen counts. */
import { expect, it, vi } from 'vitest';
import { ReadScheduler } from '@/api/readScheduler';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

it('shares overlapping suppliers and releases a failed supplier for an explicit retry', async () => {
  const scheduler = new ReadScheduler(); const pending = deferred<string>();
  const read = vi.fn(() => pending.promise);
  const one = scheduler.run(['overview', 'org'], read);
  const two = scheduler.run(['overview', 'org'], read);
  expect(one).toBe(two);
  await Promise.resolve(); expect(read).toHaveBeenCalledOnce();
  pending.resolve('native page'); expect(await two).toBe('native page');
  await expect(scheduler.run(['failed'], () => Promise.reject(new Error('offline')))).rejects.toThrow('offline');
  await expect(scheduler.run(['failed'], () => Promise.resolve('repaired'))).resolves.toBe('repaired');
});

it('retires an old reader even when its transport ignores cancellation and finishes last', async () => {
  const scheduler = new ReadScheduler(); const old = deferred<string>(); const published: string[] = [];
  const former = scheduler.run(['overview'], async (current) => { const data = await old.promise; if (current()) published.push(data); return data; });
  await Promise.resolve(); scheduler.advance();
  await scheduler.run(['overview'], (current) => { if (current()) published.push('current'); return Promise.resolve('current'); });
  old.resolve('former'); await former;
  expect(published).toEqual(['current']);
});

it('one aggregate supplies exact native pages until an authority cut and keeps their repair owner', () => {
  const scheduler = new ReadScheduler(); const owner = ['org-overview', 'org', { participating: false }];
  const page = ['children', 'org', 'exclude', false]; const visits = ['names', 'thread', '', 50, 'org', false, false];
  scheduler.supply(owner, [page, visits]);
  expect(scheduler.owns(page)).toBe(true); expect(scheduler.owns(visits)).toBe(true);
  expect(scheduler.owns(['children', 'org', 'exclude', true])).toBe(false);
  expect(scheduler.owns(['children', 'other', 'exclude', false])).toBe(false);
  scheduler.advance(); expect(scheduler.owns(page)).toBe(false); expect(scheduler.owner(page)).toEqual(owner);
  scheduler.supply(owner, [page]); expect(scheduler.owns(page)).toBe(true);
  scheduler.release(owner); expect(scheduler.owner(page)).toBeUndefined();
});
