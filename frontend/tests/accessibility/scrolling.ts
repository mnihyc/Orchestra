/** Screen-wide checks for nested scrolling, including the frame's generated panel wrappers. */
import { expect, type Page } from '@playwright/test';

export async function scrollingFaults(page: Page, within?: string): Promise<string[]> {
  return page.evaluate((selector) => {
    const roots = selector === null ? [document.body] : Array.from(document.querySelectorAll<HTMLElement>(selector));
    const nodes = new Set(roots.flatMap((root) => [root, ...Array.from(root.querySelectorAll<HTMLElement>('*'))]));
    const axes = new Map<HTMLElement, readonly ('x' | 'y')[]>();
    const scrolls = (element: HTMLElement): readonly ('x' | 'y')[] => {
      const known = axes.get(element);
      if (known !== undefined) return known;
      if (element.closest('[hidden],[inert],[aria-hidden="true"]') !== null || !element.getClientRects().length) { axes.set(element, []); return []; }
      const style = getComputedStyle(element);
      const result: ('x' | 'y')[] = [];
      // One CSS pixel allows fractional painting, rather than excusing another row of scrolling.
      if (/^(auto|scroll|overlay)$/.test(style.overflowX) && element.scrollWidth > element.clientWidth + 1) result.push('x');
      if (/^(auto|scroll|overlay)$/.test(style.overflowY) && element.scrollHeight > element.clientHeight + 1) result.push('y');
      axes.set(element, result);
      return result;
    };
    const area = (element: HTMLElement) => element.closest('[data-view-group],[data-workbench-bar],[data-mobile-main],[role="dialog"]') ?? document.body;
    const name = (element: HTMLElement) => `${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ''}${element.getAttribute('aria-label') ? `[${element.getAttribute('aria-label')}]` : ''}${element.hasAttribute('data-scroll-area') ? '[data-scroll-area]' : ''}`;
    const allowedContent = new Set(['code', 'table', 'diagram', 'choices']);
    const faults: string[] = [];
    for (const element of nodes) {
      if (!(element instanceof HTMLElement) || allowedContent.has(element.dataset.scrollAllow ?? '')) continue;
      for (const axis of scrolls(element)) {
        const boundary = area(element);
        for (let parent = element.parentElement; parent !== null; parent = parent.parentElement) {
          if (scrolls(parent).includes(axis)) { faults.push(`${axis}: ${name(element)} scrolls inside ${name(parent)}`); break; }
          if (parent === boundary || parent === document.body) break;
        }
      }
    }
    return faults;
  }, within ?? null);
}

export async function expectOneScrollerPerArea(page: Page, within?: string): Promise<void> {
  expect(await scrollingFaults(page, within), 'A view owns scrolling; an inner content scroller must declare its purpose.').toEqual([]);
}
