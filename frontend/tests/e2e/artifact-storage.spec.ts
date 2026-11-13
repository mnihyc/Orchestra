/** The existing Storage screen shows recorded makers, artifact kinds and sizes in both languages. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { artifactStorageReport } from '../doubles/artifact-storage';
import { asAdmin } from './admin-mode-support';
import { DOUBLE, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`retained artifact makers survive the Storage filter click (${locale}/${theme})`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    await asAdmin(page, true);
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, {
      method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }),
      signal: AbortSignal.timeout(10_000),
    });
    expect(appearance.ok).toBe(true);
    await page.route(/\/api\/v1\/storage(?:[/?]|$)/u, async route => {
      const params = new URL(route.request().url()).searchParams;
      await route.fulfill({ status: 200, json: artifactStorageReport(params.get('kind') === 'export' ? { kind: 'export' } : {}) });
    });
    await page.goto('/storage?scope=system');
    await screenReady(page, locale, theme);
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      const table = page.getByRole('table', { name: caption('@storage.records', locale), exact: true });
      for (const label of ['Workspace snapshot', 'Export package', 'reviewed.md']) await expect(table.getByText(label, { exact: true })).toBeVisible();
      await expect(table.getByText(caption('@storage.kinds.snapshot', locale), { exact: true })).toBeVisible();
      await expect(table.getByText(caption('@storage.kinds.import', locale), { exact: true })).toBeVisible();
      await expect(table.getByText('Ada', { exact: true }).first()).toBeVisible();
      await expect(table.getByText(/512\s?B/u, { exact: true })).toBeVisible();
      await page.getByRole('combobox', { name: caption('@storage.kind', locale), exact: true }).click();
      await page.getByRole('option', { name: caption('@storage.kinds.export', locale), exact: true }).click();
      await expect(table.getByText('Export package', { exact: true })).toBeVisible();
      await expect(table.getByText('Workspace snapshot', { exact: true })).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`artifact-storage-${width}.png`) });
      await page.getByRole('button', { name: caption('@storage.clearFilters', locale), exact: true }).click();
    }
  });
}
