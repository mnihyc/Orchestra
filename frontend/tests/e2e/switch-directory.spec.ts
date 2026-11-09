/** The existing placement menu, directory browser and frozen native plan are one click path. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { change, MAIN } from './change-support.ts';
import { emit, scenario } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`selects a machine-relative directory through its saved native switch plan ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await change.mode('manual'); await emit(MAIN, { thread: { state: 'idle' } });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/thread/${MAIN}`);
    await page.locator('[data-thread-more]').click();
    await page.getByRole('menuitem', { name: caption('@thread.header.placement', locale), exact: true }).hover();
    await page.getByRole('menuitem', { name: caption('@change.menu.move', locale), exact: true }).click();
    const form = page.getByRole('dialog').filter({ has: page.locator('[data-change-where]') });
    await form.getByRole('button', { name: caption('@change.directory.change', locale), exact: true }).click();
    await form.getByRole('button', { name: caption('@change.directory.browse', locale), exact: true }).click();
    await page.getByRole('button', { name: 'src', exact: true }).click();
    const planResponse = page.waitForResponse((response) => {
      if (!response.url().endsWith(`/threads/${MAIN}/forks/preflight`)) return false;
      const body: unknown = response.request().postDataJSON();
      return typeof body === 'object' && body !== null && 'directory' in body && body.directory === 'web/src';
    });
    await page.getByRole('button', { name: caption('@change.directory.use', locale), exact: true }).click();
    const response = await planResponse, plan: unknown = await response.json();
    if (typeof plan !== 'object' || plan === null || !('preflight_id' in plan) || typeof plan.preflight_id !== 'string') throw new Error('The native saved plan is required');
    await expect(form.locator('[data-plan]')).toContainText(caption('@change.directory.planned', locale).replace('{{path}}', 'web/src'));
    await page.screenshot({ path: info.outputPath('switch-directory-plan.png') });
    const submitted = page.waitForRequest((request) => request.method() === 'POST' && request.url().endsWith(`/threads/${MAIN}/forks`));
    await form.getByRole('button', { name: caption('@change.confirm.switch', locale), exact: true }).click();
    expect((await submitted).postDataJSON()).toEqual({ preflight_id: plan.preflight_id });
    await change.finish();
    await expect(form.getByRole('button', { name: caption('@change.result.doneButton', locale), exact: true })).toBeVisible();
  });
}
