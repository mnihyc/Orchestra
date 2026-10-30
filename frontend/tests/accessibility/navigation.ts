/** A controlled section list has one current destination, matching the section actually in view. */
import { expect, type Page } from '@playwright/test';

export async function navigationFaults(page: Page, within?: string): Promise<string[]> {
  return page.evaluate((selector) => {
    const roots = selector === null ? [document.body] : Array.from(document.querySelectorAll<HTMLElement>(selector));
    const lists = new Set(roots.flatMap((root) => [...(root.matches('[data-section-nav]') ? [root] : []), ...Array.from(root.querySelectorAll<HTMLElement>('[data-section-nav]'))]));
    const faults: string[] = [];
    for (const list of lists) {
      if (list.closest('[hidden],[inert],[aria-hidden="true"]') !== null || list.getClientRects().length === 0) continue;
      const current = Array.from(list.querySelectorAll<HTMLElement>('[data-section-id][aria-current="page"]'));
      const expected = list.dataset.currentSection;
      const name = list.getAttribute('aria-label') ?? 'Section list';
      if (current.length !== 1 || expected === undefined || current[0]?.dataset.sectionId !== expected) {
        faults.push(`${name}: expected ${expected ?? 'one current section'}, found ${current.map((item) => item.dataset.sectionId ?? '').join(', ') || 'none'}`);
      }
    }
    return faults;
  }, within ?? null);
}

export async function expectOneCurrentSection(page: Page, within?: string): Promise<void> {
  expect(await navigationFaults(page, within), 'Every section list marks its one current destination.').toEqual([]);
}
