/** Inspect actual per-field report sources while configured choices survive a fresh vendor observation. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { id } from '../doubles/fixtures';
import { scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const PROVIDER = id(0x22, 1);
const MODEL = 'source-evidence-model';
async function control(path: string, body: unknown) {
  const response = await fetch(`${DOUBLE}/__double${path}`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(body) });
  if (!response.ok) throw new Error(`the fixture refused ${path}: ${response.status}`);
}
const observation = (images: boolean) => ({ model_name: MODEL, display_name: 'Vendor model name', listed: true,
  parameters: { input_images: images, offered: { context_max: 200000, web_search: ['live'] } },
  origins: { '/display_name': 'vendor', '/parameters/input_images': 'vendor', '/parameters/offered/context_max': 'vendor', '/parameters/offered/web_search': 'catalogue' },
});

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`model observation sources ${language}/${theme}`, async ({ page }, info) => {
    await scenario();
    await control('/model/entry', { provider_id: PROVIDER, model_name: MODEL, display_name: 'My retained name', parameters: { input_images: true } });
    await control('/resources/model-observation', { provider_id: PROVIDER, models: [observation(false)], at: '2026-11-08T06:00:00Z' });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    for (const width of [1280, 400]) {
      await control('/resources/model-observation', { provider_id: PROVIDER, models: [observation(false)], at: '2026-11-08T06:00:00Z' });
      await page.setViewportSize({ width, height: 1000 });
      await page.goto(`/resources/providers/${PROVIDER}?tab=models`);
      const row = page.getByRole('row').filter({ has: page.getByText('My retained name', { exact: true }) });
      const writes: string[] = [];
      const watch = (request: import('@playwright/test').Request) => {
        if (['POST', 'PUT', 'PATCH', 'DELETE'].includes(request.method()) && new URL(request.url()).pathname.startsWith(`/api/v1/providers/${PROVIDER}`)) writes.push(request.url());
      };
      page.on('request', watch);
      await expect(row).toContainText(words.providerFill.vendorDiffers.replace('{{fields}}', `${words.providerFill.fields.name}, ${words.providerFill.fields.images}`));
      await row.getByRole('button', { name: words.providerFill.reportedValues, exact: true }).click();
      const card = page.getByRole('dialog', { name: words.providerFill.reportedValues, exact: true });
      const image = card.locator('[data-observed-field="images"]');
      await expect(image).toContainText(words.providerFill.no);
      await expect(image).toContainText(words.providerFill.sources.vendor);
      await expect(card.locator('[data-observed-field="search"]')).toContainText(words.providerFill.sources.catalogue);
      await expect(card.locator('time')).toHaveAttribute('datetime', '2026-11-08T06:00:00Z');
      const bounds = await card.boundingBox();
      if (bounds === null) throw new Error('the actual model source card is not visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0);
      expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`model-sources-${language}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
      await page.keyboard.press('Escape');
      await expect(card).toHaveCount(0);
      expect(writes).toEqual([]);
      await control('/resources/model-observation', { provider_id: PROVIDER, models: [observation(true)], at: '2026-11-08T07:00:00Z' });
      await page.reload();
      await expect(row).toContainText('My retained name');
      await expect(row).toContainText(words.providerFill.vendorDiffers.replace('{{fields}}', words.providerFill.fields.name));
      await row.getByRole('button', { name: words.providerFill.reportedValues, exact: true }).click();
      const refreshed = page.getByRole('dialog', { name: words.providerFill.reportedValues, exact: true });
      await expect(refreshed.locator('[data-observed-field="images"]')).toContainText(words.providerFill.yes);
      await expect(refreshed.locator('time')).toHaveAttribute('datetime', '2026-11-08T07:00:00Z');
      await page.keyboard.press('Escape');
      expect(writes).toEqual([]);
      page.off('request', watch);
      await expect(page.getByRole('button', { name: words.providerFill.sourceGuide, exact: true })).toHaveAttribute('data-help-heading', 'confirmed-values');
    }
  });
}
