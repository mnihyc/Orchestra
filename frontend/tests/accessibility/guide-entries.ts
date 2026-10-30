/** The shared fake/native screen walk rejects two visible guide entries with one destination or accessible name. */
import { expect, type Page } from '@playwright/test';
import { checkGuideEntries, type GuideEntry } from '../../src/help/entryCheck';

export async function expectUniqueGuideEntries(page: Page, within?: string): Promise<void> {
  const root = within === undefined ? page.locator('body') : page.locator(within);
  const controls = root.locator('[data-help-article]:visible');
  const surfaces = new Map<number, GuideEntry[]>();
  for (const control of await controls.all()) {
    const surface = await control.evaluate((element) => {
      if (element.closest('[inert], [hidden], [aria-hidden="true"]') !== null) return null;
      const modal = element.closest('[role="dialog"], [role="alertdialog"]');
      return modal === null ? 0 : Array.from(document.querySelectorAll('[role="dialog"], [role="alertdialog"]')).indexOf(modal) + 1;
    });
    if (surface === null) continue;
    const article = await control.getAttribute('data-help-article');
    expect(article, 'a guide entry declares its actual article').not.toBeNull();
    const name = await control.getAttribute('aria-label') ?? await control.innerText();
    await expect(control).toHaveAccessibleName(name);
    const entries = surfaces.get(surface) ?? [];
    entries.push({ article: article ?? '', heading: await control.getAttribute('data-help-heading'), name });
    surfaces.set(surface, entries);
  }
  for (const [surface, entries] of surfaces) expect(checkGuideEntries(entries), `guide entries at ${new URL(page.url()).pathname}, surface ${surface}`).toEqual([]);
}
