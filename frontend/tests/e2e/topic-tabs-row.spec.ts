/** Topic tabs stay on one scrolling row in the actual pane, with every destination reachable. */
import { expect, test, type Locator } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, scenario } from './support.ts';

async function inStrip(list: Locator, tab: Locator) {
  await expect.poll(async () => {
    const viewport = await list.boundingBox(), value = await tab.boundingBox();
    return viewport !== null && value !== null && value.x >= viewport.x - 1 && value.x + value.width <= viewport.x + viewport.width + 1;
  }).toBe(true);
}
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`topic tabs use one row and reveal the selected destination ${width}/${theme}/${locale}`, async ({ page }) => {
    await scenario(); await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/topic/${ACME.fixLogin}`);
    const list = page.getByRole('tablist', { name: caption('@places.topic.sections', locale), exact: true }), tabs = list.getByRole('tab');
    await expect(tabs).toHaveCount(7);
    const lines = await tabs.evaluateAll((items) => items.map((item) => item.getBoundingClientRect().top));
    expect(Math.max(...lines) - Math.min(...lines)).toBeLessThan(1);
    await page.screenshot({ path: test.info().outputPath('topic-tabs.png') });
    const overflow = await list.evaluate((element) => element.scrollWidth - element.clientWidth);
    if (overflow > 0) {
      await list.hover(); await page.mouse.wheel(0, overflow); await inStrip(list, tabs.last());
      await page.mouse.wheel(0, -overflow); await inStrip(list, tabs.first());
      await page.mouse.wheel(overflow, 0); await inStrip(list, tabs.last());
    }
    // Settings is a separate section page; the other six keep this tab list mounted.
    for (const tab of (await tabs.all()).slice(0, -1)) {
      await tab.click(); await expect(tab).toHaveAttribute('aria-selected', 'true'); await inStrip(list, tab);
      await expect.poll(() => new URL(page.url()).searchParams.get('tab') ?? 'overview').toBe(await tab.getAttribute('data-tab-value'));
    }
    await page.reload(); await expect(tabs.nth(5)).toHaveAttribute('aria-selected', 'true'); await inStrip(list, tabs.nth(5));
    await tabs.nth(5).focus(); await page.keyboard.press('Home'); await expect(tabs.first()).toBeFocused(); await inStrip(list, tabs.first());
    await page.keyboard.press('End');
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('settings');
    await expect(page.getByRole('heading', { name: caption('@settings.org.sectionNames.general', locale), exact: true })).toBeVisible();
    await page.goto(`/topic/${ACME.fixLogin}?tab=overview`);
    await expect(tabs).toHaveCount(7);
    await tabs.last().scrollIntoViewIfNeeded(); await inStrip(list, tabs.last()); await tabs.last().click();
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('settings');
    await expect(page.getByRole('heading', { name: caption('@settings.org.sectionNames.general', locale), exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
