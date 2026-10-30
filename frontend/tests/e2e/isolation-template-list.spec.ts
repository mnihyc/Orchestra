/** Similar template names keep enough room to be distinguished with the resource sidebar open. */
import { createInstance } from 'i18next';
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { detailTemplate } from '../component/resource-detail-fixtures';
import { inventoryRow } from '../doubles/everything-row';
import { scenario, signIn } from './support';

const names = ['KVM tools Alpha', 'KVM tools Bravo', 'Developer tools'];
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`template names remain distinct (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await signIn(page);
    const words = createInstance(); await words.init({ lng: locale, fallbackLng: false, resources: { en: { translation: english }, 'zh-CN': { translation: chinese } } });
    const t = words.getFixedT(locale);
    await page.goto('/settings/interface'); await page.getByRole('radio', { name: theme === 'dark' ? 'Dark' : 'Light', exact: true }).click();
    if (locale === 'zh-CN') {
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await page.route('**/api/v1/everything?*', async (route) => {
      const params = new URL(route.request().url()).searchParams;
      if (params.get('resource_details') !== 'true' || params.get('kinds') !== 'template') { await route.continue(); return; }
      const items = names.map((name, index) => {
        const value = { ...detailTemplate, id: `template-${index}`, name, driver: 'docker', spec: { image: 'registry.example.test/tools/developer:stable', runtime: 'runc' } };
        return inventoryRow({ id: value.id, kind: 'template', name, owner: value.owner, my_access: 'manage', resource: { kind: 'template', value } });
      });
      const response = await route.fetch(); await route.fulfill({ response, json: { items, next_cursor: null } });
    });
    await page.goto('/resources/templates');
    if (width === 1440) {
      const resources = page.locator('[data-navigation-rail]').getByRole('button', { name: t('workbench.choices.containers.resource-navigation.name'), exact: true });
      if (await resources.getAttribute('aria-pressed') !== 'true') await resources.click();
      await expect(resources).toHaveAttribute('aria-pressed', 'true');
    }
    const table = page.getByRole('table', { name: t('templates.list.label'), exact: true });
    await expect(table).toBeVisible(); await page.evaluate(async () => { await document.fonts.ready; });
    for (const name of names) {
      const link = table.getByRole('link', { name, exact: true }); await expect(link).toBeVisible();
      expect(await link.evaluate((element) => element.scrollWidth <= element.clientWidth + 1), name).toBe(true);
    }
    const identity = await table.locator('tbody [data-table-column="name"]').first().boundingBox();
    expect(identity?.width).toBeGreaterThanOrEqual(192);
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await info.attach(`templates-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
  });
}
