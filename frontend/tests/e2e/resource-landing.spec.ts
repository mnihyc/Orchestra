/** A typed resource root is a working landing and starts only its own summary, at both widths and languages. */
import { expect, test } from '@playwright/test';
import { ACME, scenario, signIn } from './support.ts';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

test.beforeEach(async () => { await scenario(); });

for (const width of [400, 1440]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`resource landing and request isolation at ${width}/${locale}/${theme}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const faults: string[] = [];
    page.on('pageerror', (error) => faults.push(error.message));
    const reads: string[] = [];
    page.on('request', (request) => { if (request.method() === 'GET' && request.url().includes('/api/v1/')) reads.push(new URL(request.url()).pathname.replace('/api/v1', '')); });
    const summary = page.waitForResponse((response) => response.url().endsWith('/api/v1/resources/summary'));
    await page.goto('/resources');
    expect((await summary).status()).toBe(200);
    const landing = page.locator('[data-resource-landing]');
    await expect(page.getByRole('heading', { level: 1, name: words.screens.sections.resources, exact: true })).toBeVisible();
    await expect(landing).toBeVisible();
    await expect(landing.getByRole('listitem')).toHaveCount(8);
    await expect(landing.getByRole('button')).toHaveCount(8);
    for (const add of await landing.getByRole('button').all()) await expect(add).toHaveText(words.common.add);
    await expect(landing.getByText('common.add', { exact: true })).toHaveCount(0);
    await expect(landing.locator('[data-resource-count][aria-busy="true"]')).toHaveCount(0);
    // A visible persistent Navigation still owns its one cold org-tree read.
    // Resource landing must not hydrate category lists, chats or another tree.
    const navigation = page.locator('[data-workbench-bar="left"]').getByRole('navigation', { name: words.views.navigation.title, exact: true });
    const navigationShown = await navigation.isVisible();
    if (navigationShown) await expect(navigation.getByRole('tree', { name: words.navigation.tree, exact: true })).toBeVisible();
    expect(reads.filter((path) => /^\/(providers|agents|machines|environments|templates|hosts|mcp-servers|mounts|everything)(\/|$)/.test(path) || /^\/orgs\/[^/]+\/(tree|chats)/.test(path))).toEqual(navigationShown ? [`/orgs/${ACME.org}/tree`] : []);
    expect(reads.filter((path) => path === '/resources/summary')).toHaveLength(1);
    expect(await landing.evaluate((element) => element.scrollWidth <= element.clientWidth + 1)).toBe(true);
    await landing.locator('[data-resource-kind="mcp"]').getByRole('link').press('Enter');
    await expect(page).toHaveURL(/\/resources\/mcp\/?$/);
    if (width === 1440) {
      const left = page.locator('[data-workbench-bar="left"]');
      const resources = page.locator('[data-navigation-rail]').getByRole('button', { name: words.navigation.resources.title, exact: true });
      if (await resources.getAttribute('aria-pressed') !== 'true') await resources.click();
      await expect(left.getByRole('navigation', { name: words.navigation.resources.title, exact: true })).toBeVisible();
      await expect(page.locator('[data-resource-navigation]')).toHaveCount(0);
      await left.getByRole('button', { name: words.resources.controls.add, exact: true }).click();
      await page.getByRole('menu').getByRole('menuitem', { name: words.screens.agents.title, exact: true }).click();
      const creation = page.getByRole('dialog', { name: words.agents.add.title, exact: true });
      await expect(creation).toBeVisible();
      await creation.getByRole('button', { name: words.common.cancel, exact: true }).click();
      await left.getByRole('button', { name: words.workbench.bars.close.left, exact: true }).click();
      const strip = page.locator('[data-resource-navigation]');
      await expect(strip).toBeVisible(); await expect(strip.getByRole('link')).toHaveCount(8);
      await resources.click();
      await expect(page.locator('[data-resource-navigation]')).toHaveCount(0);
    } else {
      await expect(page.locator('[data-resource-navigation]')).toBeVisible();
      await expect(page.locator('[data-resource-navigation]').getByRole('link')).toHaveCount(8);
    }
    const breadcrumb = page.locator('[data-screen-path]');
    await breadcrumb.getByRole('link', { name: locale === 'en' ? 'Resources' : '资源', exact: true }).click();
    await expect(page).toHaveURL(/\/resources\/?$/);
    await expect(landing).toBeVisible();
    expect(faults).toEqual([]);
  });
}
