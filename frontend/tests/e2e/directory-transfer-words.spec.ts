/** A directory transfer's real review and completed job keep the native copy lifecycle visible. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, RUNS_ON } from '../doubles/fixtures.ts';
import { change } from './change-support.ts';
import { emit, scenario } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`calls a native directory transfer a switch with a copy ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await change.mode('manual'); await emit(ACME.mainThread, { thread: { state: 'idle' } });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/resources/environments/${RUNS_ON.workspace}`);
    await page.getByRole('button', { name: caption('@actions.actions.named', locale).replace('{{name}}', 'Workspace'), exact: true }).click();
    await page.getByRole('menuitem', { name: caption('@change.env.menu.move', locale), exact: true }).click();
    const title = caption('@envTransfer.copyTitle', locale).replace('{{name}}', 'Workspace');
    const form = page.getByRole('dialog', { name: title, exact: true });
    await expect(form).toBeVisible();
    await form.getByRole('checkbox', { name: caption('@changeCompact.copyDirectories', locale), exact: true }).check();
    const ends = form.getByRole('region', { name: caption('@change.lists.ends', locale), exact: true });
    await expect(ends).toContainText(caption('@envTransfer.oldKept', locale));
    await expect(ends).not.toContainText(locale === 'en' ? 'old unit' : '旧单元');
    await page.screenshot({ path: info.outputPath('directory-copy-plan.png') });
    const confirm = form.getByRole('button', { name: caption('@envTransfer.copy', locale), exact: true });
    await expect(confirm).not.toHaveAttribute('aria-disabled', 'true'); await confirm.click();
    await expect(form.getByRole('heading', { name: caption('@envTransfer.copyHeading', locale).replace('{{name}}', 'Workspace').replace('{{destination}}', 'host-1'), exact: true })).toBeVisible();
    await change.finish();
    await expect(form.getByText(caption('@envTransfer.copyDone', locale).replace('{{name}}', 'Workspace').replace('{{destination}}', 'host-1'), { exact: true })).toBeVisible();
    await expect(form.locator('[data-job-result]')).not.toContainText(locale === 'en' ? 'old unit' : '旧单元');
    await page.screenshot({ path: info.outputPath('directory-copy-done.png') });
  });
}
