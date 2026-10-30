/** Primary navigation wraps instead of disappearing behind an overflow menu. */
import { expect, type Locator, test } from '@playwright/test';

import { ACME, scenario, signIn } from './support.ts';

async function expectVisibleControls(strip: Locator, controls: Locator) {
  const box = await strip.boundingBox();
  expect(box).not.toBeNull();
  if (box === null) return;
  for (const control of await controls.all()) {
    await expect(control).toBeVisible();
    const bounds = await control.boundingBox();
    expect(bounds).not.toBeNull();
    if (bounds === null) continue;
    // One CSS pixel allows rounding of fractional borders at different device scales.
    expect(bounds.x).toBeGreaterThanOrEqual(box.x - 1);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(box.x + box.width + 1);
    expect(bounds.y).toBeGreaterThanOrEqual(box.y - 1);
    expect(bounds.y + bounds.height).toBeLessThanOrEqual(box.y + box.height + 1);
  }
  expect(await strip.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
  await expect(strip.getByRole('button', { name: /More|tabs out of view/ })).toHaveCount(0);
}

test.beforeEach(async () => { await scenario(); });

for (const width of [400, 768]) {
  test(`all topic tabs are visible and reachable by pointer and keyboard at ${width}px`, async ({ page }) => {
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`/topic/${ACME.fixLogin}`);
    const strip = page.locator('[data-navigation-strip="tabs"]').first();
    const tabs = strip.getByRole('tab');
    await expect(tabs).toHaveCount(7);
    await expectVisibleControls(strip, tabs);
    if (width === 400) {
      const first = await tabs.first().boundingBox();
      const last = await tabs.last().boundingBox();
      expect(first).not.toBeNull();
      expect(last).not.toBeNull();
      if (first !== null && last !== null) expect(last.y).toBeGreaterThan(first.y);
    }
    for (const tab of await tabs.all()) {
      await tab.click();
      await expect(tab).toHaveAttribute('aria-selected', 'true');
    }
    await tabs.first().focus();
    await tabs.first().press('End');
    await expect(tabs.last()).toHaveAttribute('aria-selected', 'true');
    await expect(tabs.last()).toBeFocused();
    await tabs.last().press('Home');
    await expect(tabs.first()).toHaveAttribute('aria-selected', 'true');
    await expect(tabs.first()).toBeFocused();
    await expectVisibleControls(strip, tabs);
  });

  test(`all settings and resource links stay visible and open their destinations at ${width}px`, async ({ page }) => {
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    for (const [path, count] of [['/org/acme/settings/general', 11], ['/resources/providers', 8], ['/settings/profile', 8]] as const) {
      await page.goto(path);
      const strip = page.locator('[data-navigation-strip="links"]').first();
      const links = strip.getByRole('link');
      await expect(links).toHaveCount(count);
      await expectVisibleControls(strip, links);
      const destination = await links.last().getAttribute('href');
      expect(destination).not.toBeNull();
      await links.last().click();
      await expect.poll(() => new URL(page.url()).pathname).toBe(destination);
      await expectVisibleControls(strip, links);
      // Arrow-key navigation includes links on every wrapped line.
      await links.first().focus();
      await links.first().press('End');
      await expect(links.last()).toBeFocused();
      await links.last().press('Home');
      await expect(links.first()).toBeFocused();
    }
  });
}
