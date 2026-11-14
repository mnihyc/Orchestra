/** The head is one row that never wraps: what does not fit leaves it, the last first. */
import { describe, expect, it } from 'vitest';
import { fitCount } from '@/features/thread/rowFit';

describe('the items of a row that fit', () => {
  it('keeps every item while they all fit, counting the gaps between them', () => {
    expect(fitCount(100, [20, 30, 40], 5)).toBe(3);
    // 20 + 5 + 30 + 5 + 40 = 100: exactly the room.
    expect(fitCount(99, [20, 30, 40], 5)).toBe(2);
  });

  it('lets the last item go first and keeps the ones before it', () => {
    expect(fitCount(60, [20, 30, 40], 5)).toBe(2);
    expect(fitCount(30, [20, 30, 40], 5)).toBe(1);
  });

  it('keeps none when the first does not fit, and every one when there is nothing to measure', () => {
    expect(fitCount(10, [20, 30], 5)).toBe(0);
    expect(fitCount(0, [], 5)).toBe(0);
    expect(fitCount(50, [0, 0, 0], 4)).toBe(3);
  });

  it('does not let a later, narrower item stay when an earlier one had to go', () => {
    expect(fitCount(55, [20, 40, 10], 5)).toBe(1);
  });
});
