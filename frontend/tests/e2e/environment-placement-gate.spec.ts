/** Header and blocked recovery consume one native placement gate, without requiring binding management. */
import { expect, test } from '@playwright/test';
import { caption } from '../../e2e/capture/open.ts';
import { signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { ACME, deny, emit, scenario } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`native environment placement remains available without thread management ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await deny('ada', ['thread.manage']);
    await emit(ACME.mainThread, { thread: { state: 'blocked', lock: null, current_turn_id: null, starting_step: 'environment', blocked: { reason: 'environment_unavailable', detail: 'The environment is unavailable', sandbox: null } } });
    await signInScreenRoute(page, { width, theme, locale }); await page.goto(`/thread/${ACME.mainThread}`);
    const recovery = page.locator('[data-environment-start-recovery]');
    await expect(recovery.getByRole('button', { name: caption('@startRecovery.elsewhere', locale), exact: true })).toBeVisible();
    await page.locator('[data-thread-more]').click();
    await page.getByRole('menuitem', { name: caption('@thread.header.placement', locale), exact: true }).hover();
    await expect(page.getByRole('menuitem', { name: caption('@change.menu.move', locale), exact: true })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: caption('@change.menu.switch', locale), exact: true })).toHaveCount(0);
    await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
    await recovery.getByRole('button', { name: caption('@startRecovery.elsewhere', locale), exact: true }).click();
    await expect(page.getByRole('dialog').filter({ has: page.locator('[data-change-where]') })).toBeVisible();
    await page.screenshot({ path: info.outputPath('native-placement-recovery.png') });
  });
}
