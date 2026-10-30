/** Both lifecycle confirmations remove their completed resource from the list; leased timing has native proofs. */
import { expect, type Page, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

async function askDelete(page: Page, words: typeof english | typeof chinese, name: string) {
  await page.getByRole('button', { name: words.resources.act.menuOf.replace('{{name}}', name), exact: true }).click();
  await page.getByRole('menuitem', { name: words.trace.removeEntry, exact: true }).click();
  return page.getByRole('dialog', { name: words.trace.removeTitle.replace('{{name}}', name), exact: true });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`deletes a managed unit and a directory registration with the right promise (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario(); await page.emulateMedia({ colorScheme: theme }); await page.setViewportSize({ width, height: 900 }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.getByRole('combobox', { name: '语言', exact: true })).toContainText('简体中文');
    }
    const words = locale === 'en' ? english : chinese;
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    await page.goto(`/resources/templates/${RUNS_ON.defaultTemplate}`);
    await page.getByRole('button', { name: words.environments.make.here, exact: true }).click();
    const make = page.getByRole('dialog', { name: words.environments.make.title, exact: true });
    const name = `Deletion ${locale} ${theme} ${width}`;
    await make.getByRole('textbox', { name: words.environments.make.name, exact: true }).fill(name);
    await make.getByRole('button', { name: words.environments.make.submit, exact: true }).click();
    await expect(page.getByRole('heading', { name, level: 1, exact: true })).toBeVisible();
    const managed = await askDelete(page, words, name);
    await expect(managed.getByText(words.trace.removalUnit, { exact: true })).toBeVisible();
    await expect(managed.getByText(words.trace.removalConnected, { exact: true })).toHaveCount(0);
    await expect(managed.getByText(words.environmentData.purge, { exact: true })).toBeVisible();
    await info.attach(`delete-unit-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await managed.getByRole('textbox', { name: words.environments.remove.type.replace('{{name}}', name), exact: true }).fill(name);
    const accepted = page.waitForResponse((response) => response.request().method() === 'DELETE' && /\/api\/v1\/environments\/[^/]+(?:\?|$)/.test(response.url()));
    await managed.getByRole('button', { name: words.trace.removeConfirm.replace('{{name}}', name), exact: true }).click();
    expect((await accepted).status()).toBe(202);
    await expect(page).toHaveURL(/\/resources\/environments(?:\?|$)/);
    await expect(page.getByRole('link', { name, exact: true })).toHaveCount(0);
    await page.reload(); await expect(page.getByRole('heading', { name: words.screens.environments.title, level: 1, exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name, exact: true })).toHaveCount(0);
    // The offline directory has no thread and is owned by this person; its files never need a removal job.
    await page.goto(`/resources/environments/${RUNS_ON.docsCheckout}`);
    await expect(page.getByRole('heading', { name: 'Docs checkout', level: 1, exact: true })).toBeVisible();
    const directory = await askDelete(page, words, 'Docs checkout');
    await expect(directory.getByText(words.trace.removalConnected, { exact: true })).toBeVisible();
    await expect(directory.getByText(words.trace.removalUnit, { exact: true })).toHaveCount(0);
    await info.attach(`delete-registration-${locale}-${theme}-${width}.png`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await directory.getByRole('textbox', { name: words.environments.remove.type.replace('{{name}}', 'Docs checkout'), exact: true }).fill('Docs checkout');
    await directory.getByRole('button', { name: words.trace.removeConfirm.replace('{{name}}', 'Docs checkout'), exact: true }).click();
    await expect(page).toHaveURL(/\/resources\/environments(?:\?|$)/);
    await page.reload(); await expect(page.getByRole('heading', { name: words.screens.environments.title, level: 1, exact: true })).toBeVisible();
    await expect(page.getByRole('link', { name: 'Docs checkout', exact: true })).toHaveCount(0);
  });
}
