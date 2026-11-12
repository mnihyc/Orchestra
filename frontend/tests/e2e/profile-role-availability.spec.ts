/** The real profile line distinguishes a directory read failure and retries it in place. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`membership role retry ${locale}/${theme}`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    let failing = true;
    await page.route(`**/api/v1/orgs/${ACME.org}/members?*`, async (route) => {
      const url = new URL(route.request().url());
      if (url.searchParams.get('q') !== 'ada' || !failing) { await route.continue(); return; }
      await route.fulfill({ status: 503, json: { type: '', code: 'internal', status: 503, title: 'Unavailable', detail: 'Directory temporarily unavailable', request_id: 'membership-retry' } });
    });
    for (const width of [1280, 400]) {
      failing = true;
      await page.setViewportSize({ width, height: 1000 });
      await page.goto('/u/ada');
      const row = page.locator(`[data-person-membership="${ACME.org}"]`);
      await expect(row).toContainText(words.implicitAvailability.roleFailed);
      await expect(row.getByRole('link')).toBeVisible();
      failing = false;
      await row.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
      await expect(row).not.toContainText(words.implicitAvailability.roleFailed);
      await expect(row.getByRole('button', { name: words.common.tryAgain, exact: true })).toHaveCount(0);
      const bounds = await row.boundingBox();
      if (bounds === null) throw new Error('the native profile membership line is visible');
      expect(bounds.x).toBeGreaterThanOrEqual(0); expect(bounds.x + bounds.width).toBeLessThanOrEqual(width + 1);
      await info.attach(`membership-role-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    }
  });
}
