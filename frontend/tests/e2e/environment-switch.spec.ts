/** A real menu-to-plan-to-completion path retains the environment's identity in both languages. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { change, MAIN, openHistory } from './change-support';
import { scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`moves a thread into its named environment from the existing menu (${locale}/${theme}/${width})`, async ({ page }) => {
    await scenario(); await change.mode('manual');
    await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme });
    await openHistory(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
      await page.goto(`/thread/${MAIN}`);
    }
    const words = locale === 'en' ? english : chinese;
    const header = page.locator('[data-thread-header]');
    await header.getByRole('button', { name: words.actions.actions.named.replace('{{name}}', 'Main thread'), exact: true }).click();
    await page.getByRole('menuitem', { name: words.thread.header.placement, exact: true }).hover();
    await page.getByRole('menuitem', { name: words.change.menu.move, exact: true }).click();
    const form = page.getByRole('dialog', { name: words.change.title.switch.replace('{{name}}', 'Main thread'), exact: true });
    await form.getByRole('combobox', { name: words.change.where.label, exact: true }).click();
    await page.getByRole('option', { name: words.changeCompact.destination.new, exact: true }).click();
    await form.getByRole('textbox', { name: words.change.where.name }).fill('Moved work');
    const confirm = form.getByRole('button', { name: words.change.confirm.switch, exact: true });
    await expect(confirm).toBeEnabled();
    await expect(confirm).not.toHaveAttribute('aria-disabled', 'true');
    await expect(form.locator('[data-plan]')).toContainText('Moved work');
    await expect(form.locator('[data-plan]')).not.toHaveAttribute('data-planning', 'true');
    await confirm.click();
    const heading = locale === 'en' ? 'Switching to Moved work on host-1' : '正在切换到 host-1 上的 Moved work';
    await expect(form.getByRole('heading', { name: heading, exact: true })).toBeVisible();
    await change.finish();
    const done = locale === 'en' ? 'This thread now runs in Moved work.' : '此线程现在在 Moved work 中运行。';
    await expect(form.getByText(done, { exact: true })).toBeVisible();
    await form.getByRole('button', { name: words.change.result.doneButton, exact: true }).click();
    const history = page.getByRole('log', { name: words.timeline.label, exact: true });
    await expect(history.getByText(locale === 'en' ? /Ada Lovelace moved .+ to Moved work on host-1/ : /Ada Lovelace 将 .+ 迁移到 host-1 上的 Moved work/)).toBeVisible();
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
  });
}
