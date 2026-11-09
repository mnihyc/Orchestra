/** Default selection is an explicit intent; an omitted model preserves a saved choice. */
import { expect, test } from '@playwright/test';
import { askModels } from '../unit/ask-fixtures';
import { fixture, language } from './ask-support';
import { ACME, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) test(`Ask returns from a selected model to the current default (${locale})`, async ({ page, request }) => {
  // EN: Ask > Ask model > choose an API model > Send > choose Use the current
  // default > Send. The second request must carry an explicit null preference.
  // 中文：问一问 > 问一问模型 > 选择 API 模型 > 发送 > 使用当前默认模型 > 发送；
  // 第二次请求必须明确重置默认偏好，不能继续使用已保存的指定模型。
  await scenario();
  const server = await fixture(page), t = await language(page, request, locale, 'light');
  await page.goto(`/topic/${ACME.fixLogin}`);
  await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
  const panel = page.locator('[data-view-window="ask"]');
  await panel.getByRole('button', { name: t('ask.model'), exact: true }).click();
  await page.getByRole('dialog', { name: t('ask.model'), exact: true })
    .getByRole('menuitemradio', { name: 'Readable API model', exact: true }).click();
  await page.keyboard.press('Escape');
  const box = panel.getByRole('textbox', { name: t('ask.composer') });
  await box.fill('Use the chosen API model');
  await box.press('Enter');
  await expect.poll(() => server.sends.length).toBe(1);
  expect(server.sends[0]?.model).toEqual(askModels().default_model);
  await server.finish();
  await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toHaveCount(0);
  await panel.getByRole('button', { name: t('ask.model'), exact: true }).click();
  await page.getByRole('dialog', { name: t('ask.model'), exact: true })
    .getByRole('menuitemradio', { name: t('ask.defaultModel'), exact: true }).click();
  await page.keyboard.press('Escape');
  await box.fill('Use the current default now');
  await box.press('Enter');
  await expect.poll(() => server.sends.length).toBe(2);
  expect(server.sends[1]).toHaveProperty('model', null);
});
