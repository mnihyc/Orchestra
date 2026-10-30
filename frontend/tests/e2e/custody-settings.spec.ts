/** Maintenance and uninstall guidance use the real shared rows in both languages, widths and themes. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { RUNS_ON } from '../doubles/fixtures';
import { replySent, scenario, signIn } from './support';

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`maintenance and uninstall ${width}px ${theme} ${locale}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const signedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth = await signedIn;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The disposable sign-in has no access token.');
    const saved = await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme, locale } });
    expect(saved.ok()).toBe(true);
    await page.goto(`/resources/hosts/${RUNS_ON.buildHost}?tab=settings`);
    await screenReady(page, locale, theme);
    const row = page.locator('[data-setting="cleanup.unused_keep_days"]');
    await expect(row).toBeVisible();
    const term = caption('@settingRegistry.cleanup.unused_keep_days.title', locale);
    const value = row.locator('input');
    await expect(value).toHaveValue('7');
    const changed = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname.endsWith('/settings/cleanup.unused_keep_days'));
    await value.fill('0');
    // The row caption precedes the input's separately accessible label.
    await row.getByText(term, { exact: true }).first().click();
    expect((await changed).ok()).toBe(true);
    await expect(value).toHaveValue(caption('@common.off', locale));
    await value.focus();
    await expect(value).toHaveValue('0');
    await value.press('Tab');
    await expect(value).toHaveValue(caption('@common.off', locale));
    await page.screenshot({ path: info.outputPath('host-maintenance.png'), fullPage: true });
    await row.getByRole('button', { name: caption('@help.hint.label', locale).replace('{{term}}', term), exact: true }).click();
    await page.getByRole('button', { name: caption('@storageCleanup.guide', locale), exact: true }).click();
    await expect(page.getByRole('heading', { name: locale === 'en' ? 'Unused caches' : '未使用缓存', exact: true })).toBeVisible();
    await page.screenshot({ path: info.outputPath('cache-help.png'), fullPage: true });
    await page.goto(`/resources/machines/${RUNS_ON.laptop}?tab=installation`);
    await screenReady(page, locale, theme);
    const uninstall = page.getByText(/orch-connector.* uninstall$/).first();
    await expect(uninstall).toBeVisible();
    await uninstall.scrollIntoViewIfNeeded();
    const hint = page.locator(`[data-help-article="storage-cleanup"][data-help-heading="uninstall"]`);
    await expect(hint).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('machine-uninstall.png'), fullPage: true });
  });
}
