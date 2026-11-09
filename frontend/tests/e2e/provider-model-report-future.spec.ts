/** EN: Edit the model, read future provider words literally, save their original JSON.
 * 中文：编辑模型，按原样读取提供方的未知名称，保存原始 JSON。 */
import { expect, test } from '@playwright/test';
import type { ProviderModelUpdateBody } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { id } from '../doubles/fixtures';
import { FUTURE_MODEL_REPORT } from '../fixtures/model-report';
import { DOUBLE, scenario, signIn } from './support';

const PROVIDER = id(0x22, 1);
const NAME = 'Future reported model';
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`prototype-named discovery facts survive editing ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
    await scenario();
    expect((await request.post(`${DOUBLE}/__double/model/entry`, { data: {
      provider_id: PROVIDER, model_name: 'gpt-sol', display_name: NAME,
      parameters: { input_images: true, offered: FUTURE_MODEL_REPORT },
    } })).ok()).toBe(true);
    await signIn(page);
    expect((await request.post(`${DOUBLE}/__double/first-steps/appearance`, { data: { locale, theme } })).ok()).toBe(true);
    await page.setViewportSize({ width, height: 1000 });
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${PROVIDER}?tab=models`);
    await page.getByRole('button', { name: words.providers.models.menuOf.replace('{{name}}', NAME), exact: true }).click();
    await page.getByRole('menuitem', { name: words.models.list.edit, exact: true }).click();
    const editor = page.getByRole('dialog', { name: words.models.editor.title });
    const disclosure = editor.locator('summary', { hasText: words.modelReport.title });
    await disclosure.focus();
    await page.keyboard.press('Enter');
    const report = editor.getByTestId('model-report');
    await expect(report.getByText('__proto__', { exact: true })).toHaveCount(2);
    await expect(report.getByText('constructor', { exact: true })).toBeVisible();
    await expect(report.getByText('toString', { exact: true })).toBeVisible();
    await expect(report).not.toContainText('[object Object]');
    await expect(report).not.toContainText('[native code]');
    expect(await editor.evaluate(node => node.scrollWidth <= node.clientWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath(`future-model-report-${locale}-${theme}-${width}.png`) });
    const saved = page.waitForRequest(event => event.method() === 'PATCH' && event.url().includes(`/providers/${PROVIDER}/models/`));
    await editor.getByRole('button', { name: words.providers.models.save, exact: true }).click();
    expect(((await saved).postDataJSON() as ProviderModelUpdateBody).parameters?.offered).toEqual(FUTURE_MODEL_REPORT);
    await expect(editor).toHaveCount(0);
  });
}
