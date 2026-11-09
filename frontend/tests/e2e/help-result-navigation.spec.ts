/** Public help results remain wheel-scrollable and open real articles from a page or a retained form. */
import { expect, test, type Locator, type Page } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, scenario } from './support.ts';

async function readLastResult(page: Page, surface: Locator, locale: 'en' | 'zh-CN', pageRoute: boolean) {
  const search = surface.getByRole('searchbox', { name: caption('@help.search.label', locale), exact: true });
  await search.fill(locale === 'en' ? 'model' : '模型');
  const results = surface.locator('[data-help-result]');
  await expect(results.first()).toBeVisible();
  const count = await results.count(), last = results.last(), list = results.first().locator('..');
  const room = await surface.boundingBox(), rows = await list.boundingBox();
  if (room === null || rows === null) throw new Error('The actual help surface and result list must have bounds.');
  const top = Math.max(room.y, rows.y), bottom = Math.min(room.y + room.height, rows.y + rows.height, page.viewportSize()?.height ?? 0);
  await page.mouse.move(rows.x + rows.width / 2, (top + bottom) / 2);
  await page.mouse.wheel(0, await list.evaluate((element) => element.scrollHeight));
  await expect(last.getByRole('button')).toBeInViewport();
  const article = await last.getAttribute('data-help-result');
  if (article === null) throw new Error('The result must retain its canonical article identity.');
  await last.getByRole('button').click();
  if (pageRoute) await expect.poll(() => new URL(page.url()).pathname).toBe(`/help/${article}`);
  await expect(surface.locator('[role="region"][tabindex="0"]').getByRole('heading', { level: 1 }).first()).toBeVisible();
  // Search works while reading too, without first leaving the selected article.
  await search.fill(locale === 'en' ? 'model' : '模型');
  await expect(results).toHaveCount(count);
}

for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`help search reaches its final article and retains the original setup ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await signInScreenRoute(page, { width, theme, locale });
    await page.goto('/help?part=articles');
    const publicHelp = page.getByRole('region', { name: caption('@views.help.title', locale), exact: true })
      .filter({ has: page.getByRole('searchbox', { name: caption('@help.search.label', locale), exact: true }) }).last();
    await readLastResult(page, publicHelp, locale, true);
    await page.goto(`/topic/${ACME.fixLogin}`);
    const starter = page.locator('[data-quick-start]');
    await starter.getByRole('textbox').fill('Keep this setup while reading');
    await starter.getByRole('button', { name: caption('@start.quick.change', locale), exact: true }).click();
    const title = caption('@start.title', locale).replace('{{parent}}', 'Fix login');
    const form = page.getByRole('dialog', { name: title, exact: true });
    await expect(form).toBeVisible(); await page.keyboard.press('F1');
    const reading = page.locator('[data-help-reading]'); await expect(reading).toBeVisible();
    await readLastResult(page, reading, locale, false);
    await page.screenshot({ path: info.outputPath('help-results-after-wheel.png') });
    await reading.getByRole('button', { name: caption('@helpFlow.back', locale), exact: true }).click();
    await expect(form).toBeVisible();
    await form.getByRole('button', { name: caption('@common.cancel', locale), exact: true }).click();
    await expect(starter.getByRole('textbox')).toHaveValue('Keep this setup while reading');
  });
}
