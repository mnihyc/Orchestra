/** A page of settings carries one finder of sections, shows its first setting on the first screen at phone width, and draws choices whole. */
import { expect, type Locator, type Page, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support';
import { signInForManagement } from './admin-mode-support';

const finders = (page: Page) => page.getByRole('searchbox', { name: 'Find a section', exact: true });

/** Whether the first setting's own control is drawn inside the window, below the page's lists and search. */
async function firstControlOnScreen(page: Page, rows: Locator): Promise<boolean> {
  const control = rows.first().locator('input, button[role="switch"], [role="combobox"], button[data-setting-editor-trigger]').first();
  await expect(control).toBeVisible();
  const box = await control.boundingBox();
  const height = page.viewportSize()?.height ?? 0;
  return box !== null && box.y + box.height <= height;
}

for (const width of [1440, 400]) {
  test(`Management Settings has one section finder and its first setting on the first screen at ${width}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await signInForManagement(page);
    await page.goto('/admin/settings');
    const content = page.locator('[data-management-content]');
    const rows = content.locator('[data-setting-row]');
    await expect(rows.first()).toBeVisible();
    // Management's own sections keep their finder; the settings' groups are found through Search settings.
    await expect(page.getByRole('navigation', { name: 'Groups of settings', exact: true }).getByRole('searchbox')).toHaveCount(0);
    expect(await finders(page).count()).toBeLessThanOrEqual(1);
    expect(await firstControlOnScreen(page, rows)).toBe(true);
    if (width === 400) {
      // The strip of Management's sections starts with a whole name, not the tail of one cut by the edge.
      const strip = page.getByRole('navigation', { name: 'Management sections', exact: true }).locator('ul');
      const cut = await strip.evaluate((list) => {
        const frame = list.getBoundingClientRect();
        const shown = [...list.querySelectorAll('a')].map((link) => link.getBoundingClientRect()).filter((box) => box.right > frame.left + 1 && box.left < frame.right - 1);
        return shown.length > 0 && (shown[0]?.left ?? 0) < frame.left - 1;
      });
      expect(cut).toBe(false);
    }
  });

  test(`a topic's Defaults show their first setting on the first screen with no finder beside Search settings at ${width}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await signIn(page);
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=defaults`);
    const rows = page.locator('[data-setting-row]');
    await expect(rows.first()).toBeVisible();
    await expect(finders(page)).toHaveCount(0);
    expect(await firstControlOnScreen(page, rows)).toBe(true);
  });

  test(`a choice that follows the system shows both its words and its value whole at ${width}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: width === 400 ? 860 : 900 });
    await signIn(page);
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=auto.title`);
    const row = page.locator('[data-setting-row][data-setting="auto.title"]');
    await expect(row).toBeVisible();
    const choice = row.getByRole('combobox').first();
    await expect(choice).toBeVisible();
    const shown = await choice.evaluate((input) => {
      const display = input.parentElement?.querySelector<HTMLElement>('span[aria-hidden]');
      if (display === null || display === undefined) return null;
      const parts = [...display.querySelectorAll<HTMLElement>('span')];
      const line = Number.parseFloat(getComputedStyle(display).lineHeight) || 24;
      return { lines: Math.round(display.getBoundingClientRect().height / line), cut: parts.some((part) => part.scrollWidth > part.clientWidth + 1),
        overflow: display.scrollWidth > display.clientWidth + 1, text: display.textContent };
    });
    expect(shown).not.toBeNull();
    expect(shown?.text).toMatch(/^As .+ decides/u);
    expect(shown?.lines).toBe(1);
    expect(shown?.cut).toBe(false);
    expect(shown?.overflow).toBe(false);
  });
}
