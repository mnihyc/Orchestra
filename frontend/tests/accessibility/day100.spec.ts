/** Own-part settings, personal views, tag definition and status reviews remain readable and keyboard-operable on a phone. */
import { expect, test } from '@playwright/test';

import { ACME, expectAccessibleSettled, PEOPLE, scenario, signIn } from '../e2e/support.ts';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41830'}`;

test.beforeEach(async () => { await scenario(); });

for (const language of ['en', 'zh-CN'] as const) {
  test(`${language} own controls have no serious barriers in either theme at their intended width`, async ({ page }) => {
    if (language === 'zh-CN') await page.setViewportSize({ width: 390, height: 844 });
    await fetch(`${DOUBLE}/__double/day100/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale: language }) });
    await signIn(page, PEOPLE.ada);
    await page.goto(`/project/${ACME.web}/settings?section=policies`);
    const row = page.locator('[data-setting="agent.instructions"]');
    await expect(row).toBeVisible();
    await expectAccessibleSettled(page, '[data-setting="agent.instructions"]');
    const proposed = language === 'en' ? 'Only this project\nKeeps two lines' : '只给这个项目\n保留第二行';
    await row.getByRole('textbox').fill(proposed);
    await row.getByRole('textbox').press('Control+A');
    await row.getByRole('textbox').press('ArrowRight');
    await expect(row.getByRole('textbox')).toHaveValue(proposed);

    await page.goto(`/project/${ACME.web}?view=list`);
    await page.getByRole('checkbox', { name: language === 'en' ? 'Select Fix login' : '选择Fix login', exact: true }).check();
    await page.getByLabel(language === 'en' ? 'Selected topics' : '已选话题').getByRole('button', { name: language === 'en' ? 'Mark done' : '标记为已完成', exact: true }).click();
    await expect(page.getByRole('dialog')).toBeVisible();
    await expectAccessibleSettled(page, '[role="dialog"]');
    await page.keyboard.press('Escape');
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.getByRole('button', { name: language === 'en' ? 'Edit tags' : '编辑标签', exact: true }).click();
    await page.getByLabel(language === 'en' ? 'Tag name' : '标签名称').fill(language === 'en' ? 'new keyboard tag' : '键盘新标签');
    await expectAccessibleSettled(page, '[role="dialog"]');
    const box = await page.getByRole('dialog').boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
  });
}

test('forced colours retain a visible selection mark and widget arrow navigation', async ({ page }) => {
  await signIn(page, PEOPLE.ada);
  await page.goto(`/project/${ACME.web}?view=list`);
  await page.emulateMedia({ forcedColors: 'active' });
  await page.getByRole('radio', { name: 'List', exact: true }).focus();
  await page.keyboard.press('ArrowRight');
  const board = page.getByRole('radio', { name: 'Board', exact: true });
  await expect(board).toBeFocused();
  await page.keyboard.press('Space');
  await expect(board).toBeChecked();
  await expect(board).toHaveCSS('text-decoration-line', 'underline');
});
