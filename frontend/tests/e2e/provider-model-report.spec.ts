/** EN: Provider → Models → Edit options → Model report → save, keeping reported facts and paid options apart.
 * 中文：提供方→模型→编辑选项→模型报告→保存；报告保留，付费选项不被自动启用。 */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { id } from '../doubles/fixtures';
import { MODEL_REPORT } from '../fixtures/model-report';
import { scenario, signIn } from './support';

const DOUBLE = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}`;
const PROVIDER = id(0x22, 1);
const NAME = 'Reported model';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`read and preserve the model report ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
    await scenario();
    const placed = await request.post(`${DOUBLE}/__double/model/entry`, { data: {
      provider_id: PROVIDER, model_name: 'gpt-sol', display_name: NAME,
      parameters: { input_images: true, offered: MODEL_REPORT },
    } });
    expect(placed.ok()).toBe(true);
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${PROVIDER}?tab=models`);
    await page.getByRole('button', { name: words.providers.models.menuOf.replace('{{name}}', NAME), exact: true }).click();
    await page.getByRole('menuitem', { name: words.models.list.edit, exact: true }).click();
    const editor = page.getByRole('dialog', { name: words.models.editor.title });
    await expect(editor).toBeVisible();
    const disclosure = editor.locator('summary', { hasText: words.modelReport.title });
    await disclosure.focus();
    await page.keyboard.press('Enter');
    const report = editor.getByTestId('model-report');
    await expect(report).toHaveAttribute('open', '');
    await expect(report.getByText(words.modelReport.stages.deprecated, { exact: true })).toBeVisible();
    await expect(report.getByText('128,000', { exact: true })).toBeVisible();
    await expect(report.getByText('1,000,000', { exact: true })).toBeVisible();
    await expect(report.getByText(words.modelReport.capabilities.pdf_input, { exact: true })).toBeVisible();
    await expect(report.getByText(`${words.modelReport.capabilities.effort} · ${words.modelReport.capabilities.max}`, { exact: true })).toBeVisible();
    await expect(report.getByText(`${words.modelReport.capabilities.thinking} · ${words.modelReport.capabilities.types} · ${words.modelReport.capabilities.adaptive}`, { exact: true })).toBeVisible();
    await expect(report.getByText(`${words.modelReport.capabilities.server_tools} · ${words.modelReport.capabilities.web_search}`, { exact: true })).toBeVisible();
    await expect(report.locator('time[datetime="2027-01-01T00:00:00Z"]')).toBeVisible();
    const search = editor.getByRole('group', { name: words.webSearch.title });
    await expect(search.getByRole('switch')).not.toHaveCount(0);
    for (const option of await search.getByRole('switch').all()) await expect(option).not.toBeChecked();
    const context = editor.getByRole('group', { name: words.models.editor.context.title });
    await expect(context.getByLabel(words.models.editor.context.largest)).toHaveValue('');
    expect(await editor.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await info.attach(`model-report-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    const saved = page.waitForRequest((event) => event.method() === 'PATCH' && event.url().includes(`/providers/${PROVIDER}/models/`));
    await editor.getByRole('button', { name: words.providers.models.save, exact: true }).click();
    const body = (await saved).postDataJSON() as { parameters: Record<string, unknown> };
    expect(body.parameters.offered).toEqual(MODEL_REPORT);
    expect(body.parameters.web_search).toBeUndefined();
    expect(body.parameters.context_max).toBeUndefined();
    await expect(editor).toHaveCount(0);
  });
}
