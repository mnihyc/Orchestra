/** A ticket request keeps its key through renewal; a new control attachment gets its own act. */
import { expect, test } from '@playwright/test';
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440])
for (const theme of ['light', 'dark'] as const) {
  test(`browser ticket retry and a new control act in ${locale}/${width}/${theme}`, async ({ page }, info) => {
    await scenario();
    await page.addInitScript(({ theme }) => {
      localStorage.setItem('orch.appearance', JSON.stringify({ locale: 'en', theme, density: 'comfortable' }));
    }, { theme });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const attempts: { key: string | undefined; body: string | null }[] = [];
    await page.route('**/api/v1/browsers/*/ticket', async (route) => {
      attempts.push({ key: route.request().headers()['idempotency-key'], body: route.request().postData() });
      if (attempts.length === 1) await route.fulfill({ status: 401, json: { type: 'orch:error:auth.invalid_token', code: 'auth.invalid_token', status: 401, title: 'Sign in', detail: '', request_id: 'renew-ticket' } });
      else await route.continue();
    });
    await page.goto(`/resources/environments/${RUNS_ON.workspace}?tab=browsers`);
    await expect(page.locator('[data-browsers-view]')).toBeVisible();
    await page.locator('[data-browser-row]').filter({ hasText: 'notion' }).getByRole('button').first().click();
    await expect(page.locator('[data-browser-front] canvas')).toBeVisible();
    await expect.poll(() => attempts.length).toBe(2);
    expect(attempts[0]?.key).toMatch(/^[0-9a-f-]{36}$/i);
    expect(attempts[1]).toEqual(attempts[0]);
    await page.getByRole('button', { name: locale === 'en' ? 'Drive' : '操控', exact: true }).click();
    await expect(page.locator('[data-browser-picture="driving"]')).toBeVisible();
    await expect.poll(() => attempts.length).toBe(3);
    expect(attempts[2]?.key).toMatch(/^[0-9a-f-]{36}$/i);
    expect(attempts[2]?.key).not.toBe(attempts[0]?.key);
    expect(attempts[2]?.body).not.toBe(attempts[0]?.body);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach('native-ticket-connected', { body: await page.screenshot(), contentType: 'image/png' });
  });
}
