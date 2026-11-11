/** The registered Usage view retries provider observations without claiming their View permission is absent. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { MAIN } from './change-support';
import { scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1280, 400]) {
  test(`provider observation read ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    let failing = true;
    await page.route(/\/api\/v1\/providers\/[^/?]+(?:\?.*)?$/, async (route) => {
      if (route.request().method() !== 'GET' || !failing) { await route.continue(); return; }
      await route.abort('failed');
    });
    await page.goto(`/thread/${MAIN}`);
    await showWorkbenchView(page, 'Usage');
    const view = page.locator('[data-thread-usage]');
    await expect(view.getByText(words.implicitAvailability.usageFailed, { exact: true })).toBeVisible();
    await expect(view.getByText(words.providerPlan.viewMissing, { exact: true })).toHaveCount(0);
    await info.attach(`provider-observation-failure-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    failing = false;
    await view.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    await expect(view.getByText(words.implicitAvailability.usageFailed, { exact: true })).toHaveCount(0);
  });
}
