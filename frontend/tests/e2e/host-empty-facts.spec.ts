/** Native host facts have an explicit empty state in their existing detail sections. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1280, 400]) {
  test(`empty host facts ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    expect((await page.request.post(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/request-feeds/fixtures`)).ok()).toBe(true);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    // Native host projections feed joined openings as well as later detail reads.
    expect((await page.request.post(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/resources/host-driver-facts`, {
      data: { id: RUNS_ON.buildHost, drivers: [], storages: [], kept_layers: [] },
    })).ok()).toBe(true);
    // EN: Open the host → Details → empty Drivers → Storage → empty Storage list.
    // 中文：打开主机 → 详情 → 未列出驱动 → 存储 → 未列出存储。
    await page.goto(`/resources/hosts/${RUNS_ON.buildHost}`);
    await page.locator('[data-resource-details] summary').click();
    await expect(page.getByText(words.implicitAvailability.noHostDrivers, { exact: true })).toBeVisible();
    await page.getByRole('tab', { name: words.resourceDetails.layout.storage, exact: true }).click();
    await expect(page.getByText(words.implicitAvailability.noHostStorages, { exact: true })).toBeVisible();
    const sections = page.locator('[data-section-nav]').filter({ has: page.locator('[data-section-id="layers"]') });
    const picker = sections.getByRole('combobox');
    // Section navigation follows the pane's available width, including a desktop page between two bars.
    if (await picker.isVisible()) {
      await picker.click();
      await page.getByRole('option', { name: words.keptLayers.title, exact: true }).click();
    } else await sections.getByRole('link', { name: words.keptLayers.title, exact: true }).click();
    await expect(page.getByText(words.implicitAvailability.noKeptLayers, { exact: true })).toBeVisible();
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await info.attach(`empty-host-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
  });
}
