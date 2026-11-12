/** Built-app click path for document options; the transport double is separate from native API qualification. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { bodySent, scenario, signIn } from './support';
import { DOCS, held } from './documents-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`own document summary options save, reload and inherit (${locale}/${theme}/${width})`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: 'Language', exact: true }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const copy = words.summaryOptions.document;
    const fields = words.summaryOptions.fields;
    await page.goto(`/doc/${DOCS.plan}`);
    const title = await page.getByRole('textbox', { name: words.documents.page.titleLabel, exact: true }).inputValue();
    const before = await held(DOCS.plan);
    await info.attach('document before summary options', { body: await page.screenshot(), contentType: 'image/png' });
    const open = async () => {
      await page.getByRole('main', { name: words.workbench.areas.main, exact: true }).getByRole('button', { name: words.actions.actions.named.replace('{{name}}', title), exact: true }).click();
      await page.getByRole('menuitem', { name: copy.menu, exact: true }).click();
      return page.getByRole('dialog', { name: copy.title, exact: true });
    };
    let dialog = await open();
    // EN: Actions → Summary options → inherited document interval → one optional input-token budget.
    // 中文：操作 → 摘要选项 → 查看文档默认间隔 → 单独覆盖输入词元预算。
    await expect(dialog.getByText(copy.inherited, { exact: true })).toBeVisible();
    await expect(dialog.getByText(copy.interval.replace('{{seconds}}', '300'), { exact: true })).toBeVisible();
    await expect(dialog.getByRole('switch', { name: fields.automatic, exact: true })).not.toBeChecked();
    const set = (name: string) => words.settings.editor.memberSet.replace('{{name}}', name);
    await dialog.getByRole('checkbox', { name: set(fields.on), exact: true }).check();
    await dialog.getByRole('checkbox', { name: words.choiceCatalog.automationTrigger.content_changed.name, exact: true }).check();
    await dialog.getByRole('checkbox', { name: set(fields.context_tokens), exact: true }).check();
    await dialog.getByRole('spinbutton', { name: fields.context_tokens, exact: true }).fill('48000');
    await dialog.getByRole('checkbox', { name: set(fields.output_chars), exact: true }).check();
    await dialog.getByRole('spinbutton', { name: fields.output_chars, exact: true }).fill('1200');
    await info.attach('document summary options own draft', { body: await dialog.screenshot(), contentType: 'image/png' });
    const saved = page.waitForResponse((response) => response.request().method() === 'PUT' && new URL(response.url()).pathname === `/api/v1/documents/${DOCS.plan}/summary-options`);
    await dialog.getByRole('button', { name: words.common.save, exact: true }).click();
    const response = await saved;
    expect(response.status()).toBe(200);
    expect(await bodySent(response.request())).toEqual({ options: { automatic: false, on: ['content_changed'], context_tokens: 48000, output_chars: 1200 }, expected_revision: 0 });
    expect(await response.json()).toEqual({ projection: 'receipt', target: { kind: 'document', id: DOCS.plan }, revision: 1 });
    await expect(dialog).toBeHidden();
    await page.reload();
    dialog = await open();
    await expect(dialog.getByText(copy.own, { exact: true })).toBeVisible();
    await expect(dialog.getByRole('spinbutton', { name: fields.output_chars, exact: true })).toHaveValue('1200');
    await expect(dialog.getByRole('spinbutton', { name: fields.context_tokens, exact: true })).toHaveValue('48000');
    await expect(dialog.getByRole('checkbox', { name: words.choiceCatalog.automationTrigger.content_changed.name, exact: true })).toBeChecked();
    const inherited = page.waitForResponse((reply) => reply.request().method() === 'PUT' && new URL(reply.url()).pathname === `/api/v1/documents/${DOCS.plan}/summary-options`);
    await dialog.getByRole('button', { name: copy.useDefault, exact: true }).click();
    const reset = await inherited;
    expect(reset.status()).toBe(200);
    expect(await bodySent(reset.request())).toEqual({ options: null, expected_revision: 1 });
    await expect(dialog).toBeHidden();
    dialog = await open();
    await expect(dialog.getByText(copy.inherited, { exact: true })).toBeVisible();
    await expect(dialog.getByRole('checkbox', { name: set(fields.output_chars), exact: true })).not.toBeChecked();
    await expect(dialog.getByRole('checkbox', { name: set(fields.context_tokens), exact: true })).not.toBeChecked();
    await expect(dialog.getByRole('button', { name: words.common.save, exact: true })).toBeDisabled();
    expect(await held(DOCS.plan)).toEqual(before);
    await info.attach('document summary options inherited', { body: await dialog.screenshot(), contentType: 'image/png' });
  });
}
