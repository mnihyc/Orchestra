/** Missing inherited metadata is not an unlimited budget or a required configuration step. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, configureThread, emit, scenario } from './support.ts';

for (const width of [400, 1440] as const) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`goal defaults remain inherited when their rows were not supplied ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario(); await configureThread(ACME.mainThread, { reset: true }); await emit(ACME.mainThread, { thread: { goal: null } });
    await page.route(`**/nodes/${ACME.mainThread}/settings`, async (route) => {
      const response = await route.fetch(), settings: unknown = await response.json();
      if (!Array.isArray(settings)) throw new Error('The settings reader supplies an array.');
      await route.fulfill({ response, json: settings.filter((row: unknown) => typeof row !== 'object' || row === null || !('key' in row) || typeof row.key !== 'string' || !row.key.startsWith('thread.goal.')) });
    });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/thread/${ACME.mainThread}`);
    await page.locator('[data-thread-more]').click();
    await page.getByRole('menuitem', { name: caption('@goals.menu.set', locale), exact: true }).click();
    const form = page.getByRole('dialog').filter({ has: page.getByRole('textbox', { name: caption('@goals.editor.objective', locale), exact: true }) });
    await expect(form.getByText(caption('@goals.editor.limitsInherited', locale), { exact: true })).toBeVisible();
    await expect(form.getByText(caption('@goals.editor.limitsNone', locale), { exact: true })).toHaveCount(0);
    await expect(form.getByRole('checkbox', { name: caption('@goals.editor.notify', locale), exact: true })).not.toBeChecked();
    await form.getByRole('textbox', { name: caption('@goals.editor.objective', locale), exact: true }).fill('Review the current work');
    await form.getByRole('textbox', { name: caption('@goals.editor.condition', locale), exact: true }).fill('The requested review is recorded');
    await expect(form.getByRole('button', { name: caption('@goals.editor.set', locale), exact: true })).toBeEnabled();
    await form.locator('summary').click();
    await expect(form.getByRole('spinbutton', { name: caption('@goals.editor.turns', locale), exact: true })).toHaveAttribute('placeholder', caption('@goals.editor.fromPlace', locale));
    await page.screenshot({ path: info.outputPath('goal-inherited-defaults.png') });
    await form.getByRole('button', { name: caption('@common.cancel', locale), exact: true }).click();
    await expect(form).toHaveCount(0);
  });
}
