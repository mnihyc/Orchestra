/** The real model menu opens today's schema editor, preserving the search choice and native JSON. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { SCREEN_GATE_ROUTES, SCREEN_PRESENTATIONS } from '../../e2e/layout-routes.ts';
import { openLayoutRoute, prepareLayoutRoute, signInScreenRoute } from '../../e2e/layout-fixtures.ts';
import { scenario } from './support.ts';

for (const mode of SCREEN_PRESENTATIONS) {
  test(`model search editor click path ${mode.locale}/${mode.theme}/${mode.width}`, async ({ page }, info) => {
    const row = SCREEN_GATE_ROUTES.find(row => row.fixture === 'provider-search-editor');
    if (row === undefined) throw new Error('The shared provider editor screen must be registered');
    await scenario(row.scenario ?? {});
    await prepareLayoutRoute(page, row);
    await signInScreenRoute(page, mode);
    await page.goto(row.url);
    await openLayoutRoute(page, row);
    const words = mode.locale === 'en' ? english : chinese;
    const dialog = page.getByRole('dialog', { name: words.models.editor.title, exact: true });
    const raw = dialog.locator('[data-option-schema-editor] details').filter({ hasText: words.providerFields.raw });
    await expect(raw).toHaveAttribute('open', '');
    await raw.locator('summary').click();
    await expect(dialog.getByRole('textbox', { name: words.providerFields.raw, exact: true })).toHaveCount(0);
    await raw.locator('summary').click();
    await expect(dialog.getByRole('textbox', { name: words.providerFields.raw, exact: true })).toHaveValue(/"native"/u);
    await expect(dialog.getByRole('group', { name: words.webSearch.title, exact: true }).getByRole('switch', {
      name: words.webSearch.modeSwitch.replace('{{mode}}', words.choiceCatalog.webSearch.live.name), exact: true,
    })).toBeChecked();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('model-schema-editor.png') });
  });
}
