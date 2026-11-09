/** A manager edits the real model draft through documented controls and keeps the same raw schema. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures';
import { scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  test(`model option controls compile to the saved schema in ${locale}/${width}/${theme}`, async ({ page }) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 }); await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface'); await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}?tab=models`);
    const more = page.getByRole('table').first().locator('button[data-action="actions"]').first();
    await expect(more).toBeVisible(); await more.click();
    await page.getByRole('menuitem', { name: words.models.list.edit, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.models.editor.title });
    const editor = dialog.locator('[data-option-schema-editor]'); await expect(editor).toBeVisible();
    await editor.locator('summary').filter({ hasText: words.providerFields.raw }).click();
    const raw = editor.getByRole('textbox', { name: words.providerFields.raw });
    await raw.fill(JSON.stringify({ 'openai-responses': { type: 'object', 'x-authored': 'keep', properties: { store: { type: 'boolean', const: false } } }, native: { type: 'object' } }));
    const verbosity = editor.locator('[data-option-field="text.verbosity"]');
    await verbosity.getByRole('combobox').click(); await page.getByRole('option', { name: words.providerFields.constrain, exact: true }).click();
    await verbosity.getByRole('switch', { name: words.providerFields.limitValues }).click();
    const text = await raw.inputValue(); const schema: unknown = JSON.parse(text);
    expect(schema).toMatchObject({ native: { type: 'object' }, 'openai-responses': { 'x-authored': 'keep', properties: { store: { const: false }, text: { properties: { verbosity: { type: 'string', enum: ['low', 'medium', 'high'] } } } } } });
    const sent = page.waitForRequest((request) => request.method() === 'PATCH' && /\/models\/[^/]+$/.test(new URL(request.url()).pathname));
    await dialog.getByRole('button', { name: words.providers.models.save, exact: true }).click();
    const body: unknown = (await sent).postDataJSON();
    expect(body).toMatchObject({ parameters: { request_options: schema } });
    await expect(dialog).toHaveCount(0);
  });
}
