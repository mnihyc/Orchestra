/** Shared rendered chrome checks: visible ink, layout gaps and field margins, never a second measurement tree. */
import { expect, type Page } from '@playwright/test';

import { contrast } from '../accessibility/contrast';

/** One CSS pixel covers fractional font/border rounding; it does not permit another icon or spacing size. */
const ROUNDING = 1;

export async function expectVisualChrome(page: Page): Promise<void> {
  const measured = await page.evaluate((rounding) => {
    const problems: string[] = [];
    const near = (actual: number, wanted: number) => Math.abs(actual - wanted) < rounding;
    const visible = (element: Element) => {
      const box = element.getBoundingClientRect();
      if (box.width === 0 || box.height === 0 || box.bottom <= 0 || box.top >= innerHeight || box.right <= 0 || box.left >= innerWidth) return false;
      for (let at: Element | null = element; at !== null; at = at.parentElement) {
        const style = getComputedStyle(at);
        if (style.visibility === 'hidden' || style.display === 'none' || Number(style.opacity) === 0 || at.hasAttribute('inert')) return false;
      }
      return true;
    };
    const name = (element: Element) => `${element.tagName.toLowerCase()} ${element.getAttribute('aria-label') ?? element.textContent.trim().slice(0, 60)}`;
    const background = (element: Element) => {
      for (let at: Element | null = element; at !== null; at = at.parentElement) {
        const colour = getComputedStyle(at).backgroundColor;
        if (colour !== 'transparent' && colour !== 'rgba(0, 0, 0, 0)') return colour;
      }
      return getComputedStyle(document.documentElement).getPropertyValue('--bg').trim();
    };
    for (const icon of Array.from(document.querySelectorAll<SVGSVGElement>('svg.lucide'))) {
      if (!visible(icon) || icon.closest('.avatar, .oc-drawing') !== null) continue;
      const box = icon.getBoundingClientRect(), expected = icon.closest('[data-navigation-rail]') === null ? 16 : 20;
      if (!near(box.width, expected) || !near(box.height, expected)) problems.push(`${name(icon.parentElement ?? icon)}: icon ${box.width}×${box.height}, wanted ${expected}`);
      const stroke = Number.parseFloat(getComputedStyle(icon).strokeWidth);
      if (Math.abs(stroke - 1.75) > 0.01) problems.push(`${name(icon.parentElement ?? icon)}: stroke ${stroke}, wanted 1.75`);
    }
    const labels: { name: string; colour: string; background: string }[] = [];
    for (const label of Array.from(document.querySelectorAll('h1,h2,h3,label,legend,[role="tab"],[cmdk-group-heading],[data-navigation-section-label]'))) {
      if (!visible(label)) continue;
      const style = getComputedStyle(label);
      if (style.textTransform === 'uppercase') problems.push(`${name(label)}: forced all caps`);
      if (label.hasAttribute('data-navigation-section-label')) {
        if (Number(style.fontWeight) > 400) problems.push(`${name(label)}: section label is heavier than normal`);
        labels.push({ name: name(label), colour: style.color, background: background(label) });
      }
    }
    for (const element of Array.from(document.body.querySelectorAll<HTMLElement>('*'))) {
      if (!visible(element) || element.closest('.uplot, .oc-drawing') !== null) continue;
      const style = getComputedStyle(element);
      if (style.display.includes('flex') || style.display.includes('grid')) {
        for (const [axis, value] of [['row', style.rowGap], ['column', style.columnGap]] as const) {
          const gap = Number.parseFloat(value);
          // gap-px draws a divider or a chart mark, not room between interface controls.
          if (!Number.isFinite(gap) || gap === 0 || element.classList.contains('gap-px')) continue;
          if (![4, 8, 12, 16, 24].some((step) => near(gap, step))) problems.push(`${name(element)}: ${axis} gap ${gap}`);
        }
      }
      if (element.matches('button:not([role="switch"], [role="checkbox"], [role="radio"]), input:not([type="checkbox"], [type="radio"], [type="range"], [type="color"], [type="file"], [type="hidden"]), textarea')) {
        const radii = [style.borderTopLeftRadius, style.borderTopRightRadius, style.borderBottomLeftRadius, style.borderBottomRightRadius].map(Number.parseFloat);
        // Unboxed text acts, joined edges and semantic pills/circles retain their shapes.
        if (Math.max(...radii) <= 12 && radii.some((radius) => radius !== 0 && !near(radius, 6))) problems.push(`${name(element)}: control radii ${radii.join('/')}`);
      }
    }
    const fields = Array.from(document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('input:not([type="checkbox"], [type="radio"], [type="range"], [type="color"], [type="file"], [type="hidden"]), textarea'));
    for (const field of fields) {
      if (!visible(field)) continue;
      const style = getComputedStyle(field), box = field.getBoundingClientRect();
      // An unboxed composer/search input gets its inner margin from the containing bordered frame.
      let frame: Element = field;
      for (let at: Element | null = field; at !== null; at = at.parentElement) {
        const look = getComputedStyle(at);
        if (Number.parseFloat(look.borderLeftWidth) > 0 && Number.parseFloat(look.borderRightWidth) > 0) { frame = at; break; }
        if (at.matches('main,article,[role="dialog"],body')) break;
      }
      const bounds = frame.getBoundingClientRect(), look = getComputedStyle(frame);
      const start = box.left + Number.parseFloat(style.borderLeftWidth) + Number.parseFloat(style.paddingLeft) - bounds.left - Number.parseFloat(look.borderLeftWidth);
      const end = bounds.right - Number.parseFloat(look.borderRightWidth) - box.right + Number.parseFloat(style.borderRightWidth) + Number.parseFloat(style.paddingRight);
      if (start < 12 - rounding || end < 12 - rounding) problems.push(`${name(field)}: inner margins ${start}/${end}, wanted at least 12`);
      if (field.classList.contains('input-leading-icon')) {
        const icon = field.parentElement?.querySelector('svg');
        if (icon === null || icon === undefined) problems.push(`${name(field)}: search glyph missing`);
        else {
          const gap = box.left + Number.parseFloat(style.borderLeftWidth) + Number.parseFloat(style.paddingLeft) - icon.getBoundingClientRect().right;
          if (gap < 8 - rounding) problems.push(`${name(field)}: text touches its glyph (${gap})`);
        }
      }
      if (field.classList.contains('input-trailing-action')) {
        const action = field.parentElement?.parentElement?.querySelector('button');
        if (action !== null && action !== undefined) {
          const gap = action.getBoundingClientRect().left - box.right + Number.parseFloat(style.borderRightWidth) + Number.parseFloat(style.paddingRight);
          if (gap < 8 - rounding) problems.push(`${name(field)}: text touches its clear action (${gap})`);
        }
      }
    }
    return { problems, labels };
  }, ROUNDING);
  expect(measured.problems, `Visual chrome at ${page.url()}`).toEqual([]);
  for (const label of measured.labels) expect(contrast(label.colour, label.background), label.name).toBeGreaterThanOrEqual(4.5);
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
}
