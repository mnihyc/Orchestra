/** Retained directory pressure cannot evict an owned page, grow empty metadata, or run idle upkeep. */
import { expect, it, vi } from 'vitest';
import { DirectoryBudget, DIRECTORY_BYTES_MAX, DIRECTORY_IDLE_MS, DIRECTORY_METADATA_MAX, DIRECTORY_RECORDS_MAX, DIRECTORY_ROWS_MAX } from '@/realtime/stores/directoryBudget';

it('replaces whole pages under row and byte pressure while keeping independently pinned panes intact', () => {
  let now = 0;
  const retired = vi.fn(), budget = new DirectoryBudget<string>(retired, () => ++now);
  const page = { rows: DIRECTORY_ROWS_MAX / 4, bytes: DIRECTORY_BYTES_MAX / 4, metadata: 100 };
  for (let n = 0; n < 4; n++) expect(budget.put(String(n), `page-${n}`, page)).toBe(true);
  const first = budget.pin('0'), second = budget.pin('1');
  expect(budget.put('next', 'next-page', page)).toBe(true);
  expect(budget.peek('0')).toBe('page-0'); expect(budget.peek('1')).toBe('page-1');
  expect(budget.peek('2')).toBeUndefined(); expect(retired).toHaveBeenCalledWith('2', 'page-2');
  expect(budget.usage()).toMatchObject({ rows: DIRECTORY_ROWS_MAX, bytes: DIRECTORY_BYTES_MAX });
  first?.(); second?.();
});

it('refuses an over-budget page without deleting unrelated unowned rows or changing a pinned current page', () => {
  const retired = vi.fn(), budget = new DirectoryBudget<string>(retired);
  const page = { rows: DIRECTORY_ROWS_MAX / 2, bytes: DIRECTORY_BYTES_MAX / 2, metadata: 10 };
  budget.put('pinned', 'current', page); const release = budget.pin('pinned');
  budget.put('free', 'retained', { rows: 1, bytes: 1, metadata: 1 });
  expect(budget.put('new', 'too-wide', { rows: DIRECTORY_ROWS_MAX, bytes: 1, metadata: 1 })).toBe(false);
  expect(budget.peek('free')).toBe('retained'); expect(retired).not.toHaveBeenCalled();
  expect(budget.put('pinned', 'too-big', { rows: 1, bytes: DIRECTORY_BYTES_MAX + 1, metadata: 1 })).toBe(false);
  expect(budget.peek('pinned')).toBe('current'); release?.();
});

it('bounds empty directories and opaque cursor strings separately from row payloads', () => {
  const budget = new DirectoryBudget<number>(() => undefined);
  for (let n = 0; n < DIRECTORY_RECORDS_MAX * 3; n++) budget.put(String(n), n, { rows: 0, bytes: 0, metadata: 10 });
  expect(budget.usage().records).toBe(DIRECTORY_RECORDS_MAX);
  expect(budget.put('cursor', 1, { rows: 0, bytes: 0, metadata: DIRECTORY_METADATA_MAX })).toBe(true);
  expect(budget.usage()).toEqual({ rows: 0, bytes: 0, metadata: DIRECTORY_METADATA_MAX, records: 1 });
  expect(budget.put('oversized-selector', 2, { rows: 0, bytes: 0, metadata: DIRECTORY_METADATA_MAX + 1 })).toBe(false);
});

it('expires only on actual use after the last owner releases, and a stale lease cannot retire its replacement', () => {
  let now = 0; const retired = vi.fn(), budget = new DirectoryBudget<string>(retired, () => now);
  const cost = { rows: 1, bytes: 10, metadata: 5 };
  budget.put('page', 'old', cost); const first = budget.pin('page'), second = budget.pin('page');
  now = DIRECTORY_IDLE_MS * 2; budget.expire(); expect(budget.peek('page')).toBe('old');
  first?.(); first?.(); second?.();
  now += DIRECTORY_IDLE_MS; expect(retired).not.toHaveBeenCalled();
  budget.expire(); expect(budget.peek('page')).toBeUndefined();
  budget.put('page', 'new', cost); const current = budget.pin('page');
  first?.(); second?.(); expect(budget.peek('page')).toBe('new'); current?.();
});
