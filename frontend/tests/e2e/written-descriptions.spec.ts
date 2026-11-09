/** Written descriptions survive reopening; only titles retain automatic controls. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn } from './support';
import { DOCS } from './documents-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`written descriptions and retained title controls (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const words = locale === 'en' ? english : chinese;
    const text = locale === 'en' ? 'A description written by the team' : '团队写下的描述';
    await page.goto(`/thread/${ACME.mainThread}`);
    const header = page.locator('[data-thread-header]');
    const actions = header.getByRole('button', { name: words.actions.actions.named.replace('{{name}}', 'Main thread'), exact: true });
    await actions.click();
    await page.getByRole('menuitem', { name: new RegExp(`^(${words.thread.description.add}|${words.thread.description.edit})$`) }).click();
    const dialog = page.getByRole('dialog', { name: words.thread.description.field, exact: true });
    await dialog.getByRole('textbox').fill(text);
    await dialog.getByRole('button', { name: words.thread.description.save, exact: true }).click();
    await expect(header).toContainText(text);
    await page.reload();
    await expect(header).toContainText(text);
    await actions.click();
    await page.getByRole('menuitem', { name: words.thread.header.view, exact: true }).click();
    await expect(page.getByRole('menuitem', { name: words.thread.header.resumeAuto, exact: true })).toBeVisible();
    await expect(page.getByRole('menuitem', { name: locale === 'en' ? 'Resume the automatic description' : '恢复自动描述', exact: true })).toHaveCount(0);
    await page.getByRole('menuitem', { name: words.thread.header.resumeAuto, exact: true }).click();
    await actions.click();
    await page.getByRole('menuitem', { name: words.thread.header.view, exact: true }).click();
    await expect(page.getByRole('menuitem', { name: words.thread.header.refreshAuto, exact: true })).toBeVisible();
    await page.keyboard.press('Escape');
    await page.keyboard.press('Escape');

    await page.goto(`/doc/${DOCS.plan}`);
    const description = page.getByRole('textbox', { name: words.documents.page.descriptionLabel, exact: true });
    const saved = page.waitForResponse((response) => response.request().method() === 'PATCH' && new URL(response.url()).pathname === `/api/v1/documents/${DOCS.plan}`);
    await description.fill(text);
    await description.press('Tab');
    expect((await saved).status()).toBe(200);
    await page.reload();
    await expect(description).toHaveValue(text);
    const image = info.outputPath(`written-description-${locale}-${theme}-${width}.png`);
    await page.screenshot({ path: image });
    await info.attach('written description', { path: image, contentType: 'image/png' });
  });
}
