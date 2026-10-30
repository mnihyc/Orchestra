/** Typed data cleanup uses the same shipped plan and factual result at both widths and in both catalogues. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open';
import { screenReady } from '../../e2e/layout-rules-helpers';
import { storageCleanupPlan, storageCleanupResult, storageReport, STORAGE_CLEANUP_JOB } from '../doubles/storage-report';
import { replySent, scenario, signIn } from './support';

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`cleanup plan ${width}px ${theme} ${locale}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const plan = storageCleanupPlan(['recovery']);
    const result = storageCleanupResult(plan, new Map([['recovery', 'purge']]));
    let accepted = 0;
    await page.route(/\/api\/v1\/storage(?:[/?]|$)/u, async (route) => {
      const request = route.request();
      if (request.method() === 'GET') await route.fulfill({ status: 200, json: storageReport() });
      else if (new URL(request.url()).pathname.endsWith('/preflight')) {
        expect(request.postDataJSON()).toEqual({ scope: 'me', resource: null, items: ['recovery'] });
        await route.fulfill({ status: 200, json: plan });
      } else {
        expect(request.postDataJSON()).toEqual({ plan_id: plan.id, choices: [{ id: 'recovery', choice: 'purge' }], confirmation_name: 'My storage', notify: false });
        accepted += 1;
        await route.fulfill({ status: 202, json: result });
      }
    });
    await page.route(`**/api/v1/jobs/${STORAGE_CLEANUP_JOB}`, (route) => route.fulfill({ status: 200, json: result }));
    const signedIn = page.waitForResponse((response) => response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/auth/login')).then(replySent);
    await signIn(page);
    const auth = await signedIn;
    if (typeof auth !== 'object' || auth === null || !('access_token' in auth) || typeof auth.access_token !== 'string') throw new Error('The disposable sign-in has no access token.');
    expect((await page.request.patch('/api/v1/me', { headers: { Authorization: `Bearer ${auth.access_token}` }, data: { theme, locale } })).ok()).toBe(true);
    await page.goto('/storage');
    await screenReady(page, locale, theme);
    await page.getByRole('checkbox', { name: caption('@storage.select', locale).replace('{{name}}', 'Workspace recovery'), exact: true }).check();
    await page.getByRole('button', { name: caption('@storage.clean', locale), exact: true }).click();
    const dialog = page.getByRole('dialog', { name: caption('@storage.clean', locale), exact: true });
    await expect(dialog.getByRole('table', { name: caption('@storage.cleanup.plan', locale), exact: true })).toBeVisible();
    const submit = dialog.getByRole('button', { name: caption('@storage.clean', locale), exact: true });
    await expect(submit).toBeDisabled();
    await dialog.getByRole('combobox', { name: caption('@storage.cleanup.fateFor', locale).replace('{{name}}', 'Workspace recovery'), exact: true }).click();
    await page.getByRole('option', { name: caption('@storage.cleanup.choices.purge', locale), exact: true }).click();
    const typed = dialog.getByRole('textbox', { name: caption('@storage.cleanup.typeName', locale).replace('{{name}}', 'My storage'), exact: true });
    await expect(submit).toBeDisabled();
    await typed.fill('my storage');
    await expect(submit).toBeDisabled();
    expect(accepted).toBe(0);
    await typed.fill('My storage');
    await expect(submit).toBeEnabled();
    expect(await dialog.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('cleanup-confirm.png'), fullPage: true });
    await submit.click();
    const outcomes = dialog.getByRole('table', { name: caption('@storage.cleanup.results', locale), exact: true });
    await expect(outcomes.getByText('Workspace recovery', { exact: true })).toBeVisible();
    await expect(outcomes.getByText(caption('@storage.cleanup.outcomes.removed', locale), { exact: true })).toBeVisible();
    expect(accepted).toBe(1);
    await page.screenshot({ path: info.outputPath('cleanup-result.png'), fullPage: true });
  });
}
