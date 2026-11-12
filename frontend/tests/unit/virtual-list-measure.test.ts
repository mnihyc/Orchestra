/** A row measured while the page has not yet been told where the reader is does not take the reader back to where they were. */
import { expect, it } from 'vitest';
import { compensatesMeasurement } from '@/ui/VirtualList';

const belief = (offset: number, more: Partial<{ adjustments: number; backward: boolean }> = {}) => ({ offset, adjustments: 0, backward: false, ...more });

it('makes good a row measured above the fold, as the library does, while the element is where the page believes it is', () => {
  expect(compensatesMeasurement({ start: 100, end: 400, first: true }, belief(2_000), 2_000)).toBe(true);
  expect(compensatesMeasurement({ start: 2_100, end: 2_400, first: true }, belief(2_000), 2_000)).toBe(false);
  expect(compensatesMeasurement({ start: 100, end: 400, first: false }, belief(2_000), 2_000)).toBe(true);
  // A row that spans the fold changes size below it; a reader who moves backward is not carried by what they pass.
  expect(compensatesMeasurement({ start: 1_900, end: 2_200, first: false }, belief(2_000), 2_000)).toBe(false);
  expect(compensatesMeasurement({ start: 100, end: 400, first: false }, belief(2_000, { backward: true }), 2_000)).toBe(false);
});

it('waits while a late scroll event leaves the element somewhere else than the page believes', () => {
  // The Home key's smooth scroll was reported at 2321 and has since reached the top: rows measured for the old place wait for the event.
  expect(compensatesMeasurement({ start: 100, end: 400, first: true }, belief(2_321), 0)).toBe(false);
  expect(compensatesMeasurement({ start: 100, end: 400, first: false }, belief(2_321), 0)).toBe(false);
  // Within a pixel is the same place.
  expect(compensatesMeasurement({ start: 100, end: 400, first: true }, belief(2_000), 2_000.5)).toBe(true);
  // Without an element there is nothing to disagree with.
  expect(compensatesMeasurement({ start: 100, end: 400, first: true }, belief(2_000), null)).toBe(true);
});
