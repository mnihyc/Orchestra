/** Set and reload a target's defaults through the existing scoped settings screen. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, bodySent, scenario, signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`summary target defaults survive a scoped save (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const fields = words.summaryOptions.fields;
    const set = (name: string) => words.settings.editor.memberSet.replace('{{name}}', name);
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=summary.targets`);
    const row = page.locator('[data-setting="summary.targets"]');
    await expect(row).toContainText(words.settingRegistry.summary.targets.title);
    await row.locator('[data-setting-editor-trigger]').click();
    await row.getByRole('checkbox', { name: set(fields.document), exact: true }).check();
    await expect(row.getByRole('switch', { name: fields.automatic, exact: true })).not.toBeChecked();
    await row.getByRole('checkbox', { name: set(fields.on), exact: true }).check();
    await row.getByRole('checkbox', { name: words.choiceCatalog.automationTrigger.content_changed.name, exact: true }).check();
    await row.getByRole('checkbox', { name: set(fields.output_chars), exact: true }).check();
    const output = row.getByRole('spinbutton', { name: fields.output_chars, exact: true });
    await output.fill('1200');
    const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === `/api/v1/nodes/${ACME.fixLogin}/settings/summary.targets`);
    await output.press('Control+Enter');
    const response = await saved;
    expect(response.status()).toBe(200);
    const body = await bodySent(response.request()) as { value: { document: unknown } };
    expect(body.value.document).toEqual({ automatic: false, on: ['content_changed'], output_chars: 1200 });
    await page.reload();
    await row.locator('[data-setting-editor-trigger]').click();
    await expect(row.getByRole('checkbox', { name: set(fields.document), exact: true })).toBeChecked();
    await expect(row.getByRole('switch', { name: fields.automatic, exact: true })).not.toBeChecked();
    await expect(row.getByRole('spinbutton', { name: fields.output_chars, exact: true })).toHaveValue('1200');
    for (const name of [fields.help, fields.search_results]) await expect(row.getByRole('checkbox', { name: set(name), exact: true })).toHaveCount(0);
    await info.attach('summary target defaults', { body: await row.screenshot(), contentType: 'image/png' });
  });
}


for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`chat title and catch-up options remain independent (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const fields = words.summaryOptions.fields;
    const set = (name: string) => words.settings.editor.memberSet.replace('{{name}}', name);
    await page.goto(`/topic/${ACME.fixLogin}?tab=settings&section=defaults&setting=summary.targets`);
    const row = page.locator('[data-setting="summary.targets"]');
    await info.attach('summary targets before chat title', { body: await row.screenshot(), contentType: 'image/png' });
    await row.locator('[data-setting-editor-trigger]').click();
    await row.getByRole('checkbox', { name: set(fields.chat), exact: true }).check();
    const catchUp = row.getByRole('group', { name: fields.chat, exact: true });
    await catchUp.getByRole('switch', { name: fields.automatic, exact: true }).check();
    await row.getByRole('checkbox', { name: set(fields.chat_title), exact: true }).check();
    const title = row.getByRole('group', { name: fields.chat_title, exact: true });
    await expect(title.getByRole('switch', { name: fields.automatic, exact: true })).not.toBeChecked();
    await title.getByRole('switch', { name: fields.automatic, exact: true }).check();
    await title.getByRole('checkbox', { name: set(fields.on), exact: true }).check();
    await title.getByRole('checkbox', { name: words.choiceCatalog.automationTrigger.message_posted.name, exact: true }).check();
    await title.getByRole('checkbox', { name: set(fields.output_chars), exact: true }).check();
    const output = title.getByRole('spinbutton', { name: fields.output_chars, exact: true });
    await output.fill('80');
    await info.attach('chat title options draft', { body: await row.screenshot(), contentType: 'image/png' });
    const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === `/api/v1/nodes/${ACME.fixLogin}/settings/summary.targets`);
    await output.press('Control+Enter');
    const response = await saved;
    expect(response.status()).toBe(200);
    expect(await bodySent(response.request())).toMatchObject({ value: { chat: { automatic: true }, chat_title: { automatic: true, on: ['message_posted'], output_chars: 80 } } });
    await page.reload(); await row.locator('[data-setting-editor-trigger]').click();
    await expect(title.getByRole('spinbutton', { name: fields.output_chars, exact: true })).toHaveValue('80');
    await expect(catchUp.getByRole('switch', { name: fields.automatic, exact: true })).toBeChecked();
    await info.attach('chat title options after reload', { body: await row.screenshot(), contentType: 'image/png' });
  });
}
