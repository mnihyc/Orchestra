/** `useAfterDelay`: a thing done a moment later, unless whoever asked has gone. */
import { StrictMode } from 'react';
import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useAfterDelay } from '@/lib/afterDelay';

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => { cleanup(); vi.useRealTimers(); });

it('does the thing once after its delay, and not before', () => {
  const done = vi.fn();
  const { result } = renderHook(() => useAfterDelay());
  result.current(done, 2_000);
  act(() => { vi.advanceTimersByTime(1_999); });
  expect(done).not.toHaveBeenCalled();
  act(() => { vi.advanceTimersByTime(1); });
  expect(done).toHaveBeenCalledTimes(1);
  act(() => { vi.advanceTimersByTime(10_000); });
  expect(done).toHaveBeenCalledTimes(1);
});

it('lets one wait be cancelled without touching the others', () => {
  const first = vi.fn(), second = vi.fn();
  const { result } = renderHook(() => useAfterDelay());
  const cancel = result.current(first, 1_000);
  result.current(second, 1_000);
  cancel();
  act(() => { vi.advanceTimersByTime(1_000); });
  expect(first).not.toHaveBeenCalled();
  expect(second).toHaveBeenCalledTimes(1);
});

it('does nothing for a component that has gone, and sets no wait for one that asks after it went', () => {
  const waiting = vi.fn(), late = vi.fn();
  const { result, unmount } = renderHook(() => useAfterDelay());
  const after = result.current;
  after(waiting, 2_000);
  unmount();
  expect(vi.getTimerCount()).toBe(0);
  after(late, 2_000);
  expect(vi.getTimerCount()).toBe(0);
  act(() => { vi.advanceTimersByTime(5_000); });
  expect(waiting).not.toHaveBeenCalled();
  expect(late).not.toHaveBeenCalled();
});

it('still works after the double mount of a strict tree', () => {
  const done = vi.fn();
  const { result } = renderHook(() => useAfterDelay(), { wrapper: StrictMode });
  result.current(done, 500);
  act(() => { vi.advanceTimersByTime(500); });
  expect(done).toHaveBeenCalledTimes(1);
});
