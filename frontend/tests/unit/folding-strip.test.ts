/** Container labels compact at their measured width boundary without hiding a choice. */
import { expect, it } from 'vitest';
import { labelsNeedIcons } from '@/ui/useFoldingStrip';

it('keeps full names when all measured labels fit, including an exact fit', () => {
  const widths = [80, 72, 150, 110];
  expect(labelsNeedIcons(widths, 412)).toBe(false);
  expect(labelsNeedIcons(widths, 600)).toBe(false);
  expect(labelsNeedIcons(widths, 411)).toBe(true);
});

it('uses measured labels rather than a fixed count or viewport breakpoint', () => {
  expect(labelsNeedIcons([32, 32, 32, 32, 32], 160)).toBe(false);
  expect(labelsNeedIcons([120, 80], 160)).toBe(true);
  expect(labelsNeedIcons([], 0)).toBe(false);
});
