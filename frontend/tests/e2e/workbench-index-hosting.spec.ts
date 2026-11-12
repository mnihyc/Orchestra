/** Real index pages retain their own params and search through clicks, tab focus and reload. */
import { expect, test } from '@playwright/test';
import { waitForSavedTabId } from './workbench-support';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`index pages keep their workbench host ${locale}/${theme}/${width}`, async ({ page }, info) => {
    const words = locale === 'en' ? english : chinese;
    const providers = locale === 'en' ? 'Providers' : '提供方';
    const faults: string[] = [];
    page.on('pageerror', (error) => faults.push(error.message));
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await scenario(); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const main = page.locator('[data-main-area]');
    await page.goto('/resources/providers');
    await expect(main.getByRole('heading', { name: providers, exact: true })).toBeVisible();
    await page.reload();
    await expect(main.getByRole('heading', { name: providers, exact: true })).toBeVisible();
    const mainTabs = page.getByRole('tablist', { name: words.workbench.areas.main, exact: true });
    const providerTab = mainTabs.getByRole('tab', { name: providers, exact: true, selected: true });
    const tabId = await waitForSavedTabId(page, providerTab);
    await page.goto('/org/acme');
    await expect(main.getByRole('heading', { name: 'Acme', exact: true })).toBeVisible();
    await main.getByRole('tab', { name: words.places.org.tabs.activity, exact: true }).click();
    await expect.poll(() => new URL(page.url()).searchParams.get('tab')).toBe('activity');
    await expect(main.getByRole('tab', { name: words.places.org.tabs.activity, exact: true })).toHaveAttribute('aria-selected', 'true');
    await page.reload();
    await expect(main.getByRole('tab', { name: words.places.org.tabs.activity, exact: true })).toHaveAttribute('aria-selected', 'true');
    if (width === 400) {
      const allTabs = words.workbench.allTabs.replace('{{name}}', words.workbench.areas.main);
      await page.getByRole('button', { name: allTabs, exact: true }).click();
      const target = 'commands:workspace.focus.' + tabId;
      await page.locator(`[role="option"][data-value=${JSON.stringify(target)}]`).click();
    } else await mainTabs.getByRole('tab', { name: providers, exact: true }).click();
    await expect(page).toHaveURL(/\/resources\/providers\/?(?:\?|$)/);
    await expect(main.getByRole('heading', { name: providers, exact: true })).toBeVisible();
    expect(faults).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(true);
    const screenshot = info.outputPath(`index-host-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: screenshot, fullPage: true, animations: 'disabled' });
    await info.attach('index-page-hosting', { path: screenshot, contentType: 'image/png' });
  });
}
