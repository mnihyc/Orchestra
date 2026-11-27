/** Topic tabs stay one row in the actual pane: those that do not fit are under More, and every destination stays reachable. */
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
const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`topic tabs stay one row with the rest under More ${width}/${theme}/${locale}`, async ({ page }) => {
    await scenario(); await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/topic/${ACME.fixLogin}`);
    const list = page.getByRole('tablist', { name: caption('@places.topic.sections', locale), exact: true });
    const every = list.getByRole('tab', { includeHidden: true }), shown = list.getByRole('tab');
    const more = page.getByRole('button', { name: caption('@ui.tabs.more', locale), exact: true });
    await expect(every).toHaveCount(7);
    // One row: every tab in view shares its line; at a phone's width some wait under More.
    const lines = await shown.evaluateAll((items) => items.map((item) => item.getBoundingClientRect().top));
    expect(Math.max(...lines) - Math.min(...lines)).toBeLessThanOrEqual(1);
    if (width === 400) await expect(more).toBeVisible();
    expect(await list.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    for (const tab of await shown.all()) await inStrip(list, tab);
    await page.screenshot({ path: test.info().outputPath('topic-tabs.png') });
    // Every destination is reached: a tab in the row by a press, another from More, where the chosen one joins the row.
    // Settings is a separate section page; the other six keep this tab list mounted.
    const names: Record<string, string> = { overview: '@places.topic.tabs.overview', threads: '@places.topic.tabs.threads', environments: '@places.topic.tabs.environments',
      documents: '@places.topic.tabs.documents', agents: '@people.agentChoices', activity: '@places.topic.tabs.activity' };
    for (const [value, name] of Object.entries(names)) {
      const tab = list.locator(`[data-tab-value="${value}"]`);
      if (await tab.isVisible()) await tab.click();
      else { await more.click(); await page.getByRole('menuitemradio', { name: new RegExp(`^${escaped(caption(name, locale))}`, 'u') }).click(); }
      await expect(tab).toHaveAttribute('aria-selected', 'true'); await expect(tab).toBeVisible(); await inStrip(list, tab);
      await expect.poll(() => new URL(page.url()).searchParams.get('tab') ?? 'overview').toBe(value);
    }
    const activity = list.locator('[data-tab-value="activity"]');
    await page.reload(); await expect(activity).toHaveAttribute('aria-selected', 'true'); await inStrip(list, activity);
    await activity.focus(); await page.keyboard.press('Home'); await expect(shown.first()).toBeFocused(); await inStrip(list, shown.first());
    await page.keyboard.press('End');
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('settings');
    await expect(page.getByRole('heading', { name: caption('@settings.org.sectionNames.general', locale), exact: true })).toBeVisible();
    await page.goto(`/topic/${ACME.fixLogin}?tab=overview`);
    await expect(every).toHaveCount(7);
    const settings = list.locator('[data-tab-value="settings"]');
    if (await settings.isVisible()) await settings.click();
    else { await more.click(); await page.getByRole('menuitemradio', { name: new RegExp(`^${escaped(caption('@places.topic.tabs.settings', locale))}`, 'u') }).click(); }
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('settings');
    await expect(page.getByRole('heading', { name: caption('@settings.org.sectionNames.general', locale), exact: true })).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  });
}
