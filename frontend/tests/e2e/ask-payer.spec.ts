/** Ask's payer choices retain named copy and their native saved values in both languages. */
import { expect, test } from '@playwright/test';
import type { ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { language } from './ask-support';
import { ACME, scenario, supplyDefaults } from './support';

for (const locale of ['en', 'zh-CN'] as const) test(`Ask payer choices save their native meaning (${locale})`, async ({ page, request }) => {
  // EN: Organization > Settings > Policies > Who pays for Ask > Setting’s writer
  // > Save > Person asking > Save. Both names and their explanations are readable;
  // the saved values are setter and person, while attribution remains the asker.
  // 中文：组织 > 设置 > 策略 > 问一问由谁付费 > 设置者 > 保存 > 提问者 > 保存；
  // 两项均有名称和说明，实际保存 setter 和 person，用量归属仍为提问者。
  await scenario();
  const definition: SettingDef = { key: 'ask.paid_by', title: 'Who pays for Ask', description: 'Who pays for each question.',
    scopes: ['system', 'org'], group: 'automation', class: 'policy', merge: 'replace',
    schema: { enum: ['person', 'setter'] }, default: 'person', example: null, text_key: null, keywords: [] };
  let current: ResolvedSetting = { key: definition.key, value: 'person', own: null, writable: true, locked_by: null,
    source: { scope: 'default', node: null }, parts: [] };
  const writes: string[] = [];
  await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: [definition] }));
  await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: [current] }));
  await supplyDefaults(page, ACME.research, { registry: () => [definition], settings: () => [current] });
  await page.route((url) => url.pathname === `/api/v1/nodes/${ACME.research}/settings/ask.paid_by`, async (route) => {
    expect(route.request().method()).toBe('PUT');
    const body = route.request().postDataJSON() as { value: 'person' | 'setter' };
    writes.push(body.value);
    current = { ...current, value: body.value, own: { value: body.value, revision: writes.length, locked: false },
      source: { scope: 'org', node: { id: ACME.research, kind: 'org', name: 'Research', redacted: false } } };
    await route.fulfill({ json: current });
  });
  await language(page, request, locale, 'light');
  const words = locale === 'en' ? english : chinese;
  await page.goto('/org/acme.research/settings/defaults');
  const row = page.locator('[data-setting="ask.paid_by"]');
  const choice = row.getByRole('combobox', { name: words.settingRegistry.ask.paid_by.title, exact: true });
  for (const value of ['setter', 'person'] as const) {
    await choice.click();
    const option = page.getByRole('option', { name: words.choiceCatalog.askPayer[value].name, exact: true });
    await expect(option).toBeVisible();
    await expect(page.getByText(words.choiceCatalog.askPayer[value].description, { exact: true })).toBeVisible();
    await option.click();
    await row.getByRole('button', { name: words.common.save, exact: true }).click();
    await expect.poll(() => writes.at(-1)).toBe(value);
  }
  expect(writes).toEqual(['setter', 'person']);
});
