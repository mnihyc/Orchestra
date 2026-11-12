/** Tabs scroll on one row; section links keep their own complete navigation. */
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
  test(`all topic tabs stay on one row and are reachable by pointer and keyboard at ${width}px`, async ({ page }) => {
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    await page.goto(`/topic/${ACME.fixLogin}`);
    const strip = page.locator('[data-navigation-strip="tabs"]').first();
    const tabs = strip.getByRole('tab');
    await expect(tabs).toHaveCount(7);
    const list = strip.getByRole('tablist');
    const positions = await tabs.evaluateAll((items) => items.map((item) => item.getBoundingClientRect().top));
    expect(Math.max(...positions) - Math.min(...positions)).toBeLessThan(1);
    const overflow = await list.evaluate((element) => element.scrollWidth - element.clientWidth);
    if (overflow > 0) { await list.hover(); await page.mouse.wheel(overflow, 0); }
    for (const tab of (await tabs.all()).slice(0, -1)) {
      await tab.click();
      await expect(tab).toHaveAttribute('aria-selected', 'true');
    }
    await tabs.first().focus();
    await tabs.nth(5).press('Home');
    await expect(tabs.first()).toHaveAttribute('aria-selected', 'true');
    await expect(tabs.first()).toBeFocused();
    const visible = await tabs.first().boundingBox(), viewport = await list.boundingBox();
    expect(visible).not.toBeNull(); expect(viewport).not.toBeNull();
    if (visible !== null && viewport !== null) {
      expect(visible.x).toBeGreaterThanOrEqual(viewport.x - 1);
      expect(visible.x + visible.width).toBeLessThanOrEqual(viewport.x + viewport.width + 1);
    }
    // End includes Settings, which opens its own section page and replaces this list.
    await tabs.first().press('End');
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('settings');
    await expect(page.getByRole('heading', { name: 'General', exact: true })).toBeVisible();
    await page.goto(`/topic/${ACME.fixLogin}?tab=overview`);
    await tabs.last().scrollIntoViewIfNeeded(); await tabs.last().click();
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('settings');
    await expect(page.getByRole('heading', { name: 'General', exact: true })).toBeVisible();
  });

  test(`all settings and resource destinations remain reachable at ${width}px`, async ({ page }) => {
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    const scenes = [
      { path: '/org/acme/settings/general', label: 'Sections of the settings of Acme', count: 11, resources: false },
      { path: '/resources/providers', label: 'Kinds of resources', count: 9, resources: true },
      { path: '/settings/profile', label: 'Your settings', count: 10, resources: false },
    ];
    for (const { path, label, count, resources } of scenes) {
      await page.goto(path);
      const nav = page.getByRole('navigation', { name: label, exact: true });
      await expect(nav).toBeVisible();
      const strip = resources ? nav.locator('[data-navigation-strip="links"]') : nav;
      const links = strip.getByRole('link');
      const anchors = resources ? links : nav.locator('a[data-section-id]');
      await expect(anchors).toHaveCount(count);
      const destination = await anchors.last().getAttribute('href');
      if (destination === null) throw new Error('The last section must have a native destination');
      const chooser = nav.getByRole('combobox', { name: label, exact: true });
      const usesChooser = await chooser.isVisible();
      if (usesChooser) {
        // Narrow settings use a searchable chooser backed by the same scoped links.
        await expectVisibleControls(nav, chooser);
        await chooser.click();
        const options = page.getByRole('listbox', { name: label, exact: true }).getByRole('option');
        await expect(options).toHaveCount(count);
        await chooser.press('End');
        await expect(options.last()).toHaveAttribute('aria-selected', 'true');
        await expect(options.last()).toBeInViewport();
        await chooser.press('Home');
        await expect(options.first()).toHaveAttribute('aria-selected', 'true');
        await expect(options.first()).toBeInViewport();
        await options.last().click();
      } else {
        await expect(links).toHaveCount(count);
        await expectVisibleControls(strip, links);
        await links.first().focus();
        if (resources) {
          await links.first().press('End');
          await expect(links.last()).toBeFocused();
          await links.last().press('Home');
          await expect(links.first()).toBeFocused();
        } else {
          for (let index = 1; index < count; index++) await page.keyboard.press('Tab');
          await expect(links.last()).toBeFocused();
        }
        await links.last().click();
      }
      await expect.poll(() => new URL(page.url()).pathname).toBe(new URL(destination, page.url()).pathname);
      const controls = usesChooser ? chooser : links;
      await expect(controls).toHaveCount(usesChooser ? 1 : count);
      await expectVisibleControls(strip, controls);
    }
  });
}
