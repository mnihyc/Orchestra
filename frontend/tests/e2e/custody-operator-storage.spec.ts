/** Operator-owned files use the common plan and copyable server commands in the shipped application. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { operatorStoragePlan, operatorStorageReport } from '../doubles/storage-report';
import { asAdmin } from './admin-mode-support';
import { replySent, scenario, signIn } from './support';

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`operator cleanup ${width}px ${theme} ${locale}`, async ({ page, context }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    let starts = 0;
    const plan = operatorStoragePlan();
    await page.route(/\/api\/v1\/storage(?:[/?]|$)/u, async (route) => {
      const request = route.request();
      if (request.method() === 'GET') {
        expect(new URL(request.url()).searchParams.get('scope')).toBe('system');
        await route.fulfill({ status: 200, json: operatorStorageReport() });
      } else if (new URL(request.url()).pathname.endsWith('/preflight')) {
        expect(request.postDataJSON()).toEqual({ scope: 'system', resource: null, items: plan.items.map((item) => item.record.id) });
        await route.fulfill({ status: 200, json: plan });
      } else {
        starts += 1;
        await route.fulfill({ status: 500, json: { message: 'Operator files have no API erase action' } });
      }
    });
    const signedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth = await signedIn;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The disposable sign-in has no access token.');
    await asAdmin(page, true);
    expect((await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme, locale } })).ok()).toBe(true);
    await page.goto('/storage?scope=system');
    await screenReady(page, locale, theme);
    await expect(page.getByText('orch-backup.dump', { exact: true })).toBeVisible();
    await page.getByRole('checkbox', { name: caption('@storage.selectAll', locale), exact: true }).check();
    await page.getByRole('button', { name: caption('@storage.clean', locale), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: caption('@storage.clean', locale), exact: true });
    await expect(dialog.getByRole('combobox')).toHaveCount(0);
    await expect(dialog.getByRole('button', { name: caption('@storage.clean', locale), exact: true })).toHaveCount(0);
    const copies = dialog.getByRole('button', { name: caption('@storage.cleanup.copyOperatorCommand', locale), exact: true });
    await expect(copies).toHaveCount(2);
    // Shared bounded identities reserve twelve character widths; commands must retain that room too.
    for (const code of await dialog.locator('pre').all()) {
      expect(await code.evaluate((element) => {
        const canvas = document.createElement('canvas');
        const context = canvas.getContext('2d');
        if (context === null) throw new Error('The browser can measure the shared command font.');
        context.font = getComputedStyle(element).font;
        return element.getBoundingClientRect().width >= context.measureText('000000000000').width;
      })).toBe(true);
    }
    for (const [index, item] of plan.items.entries()) {
      await expect(dialog.getByText(item.record.operator_command ?? '', { exact: true })).toBeVisible();
      await copies.nth(index).click();
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(item.record.operator_command);
    }
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    expect(starts).toBe(0);
    await page.screenshot({ path: info.outputPath('operator-cleanup.png'), fullPage: true });
    await dialog.locator('footer').getByRole('button', { name: caption('@common.close', locale), exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(starts).toBe(0);
  });
}
