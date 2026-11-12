/** The existing settings editor carries image modes; these fixtures prove presentation and guarded writes. */
import { expect, test } from '@playwright/test';
import type { ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn, supplyDefaults } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`image generation keeps program default until chosen ${locale}/${theme}/${width}`, async ({ page }) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const definition: SettingDef = {
      key: 'agents.image_generation', title: 'Image generation', description: 'Program default when unset.',
      scopes: ['org'], group: 'agent', class: 'parameter', merge: 'replace',
      schema: { anyOf: [{ type: 'string', enum: ['off', 'on', 'service'] }, { type: 'null' }] },
      default: null, example: 'off', text_key: null, keywords: [],
    };
    let current: ResolvedSetting = { key: definition.key, value: null, own: null,
      writable: true, locked_by: null, source: { scope: 'default', node: null }, parts: [] };
    const writes: { value: 'off' | 'on' | 'service' | null; expected_revision: number }[] = [];
    await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: [definition] }));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: [current] }));
    await supplyDefaults(page, ACME.research, { registry: () => [definition], settings: () => [current] });
    await page.route((url) => url.pathname === `/api/v1/nodes/${ACME.research}/settings/${definition.key}`, async (route) => {
      expect(route.request().method()).toBe('PUT');
      const body = route.request().postDataJSON() as { value: 'off' | 'on' | 'service' | null; expected_revision: number };
      writes.push(body);
      current = { ...current, value: body.value, own: { value: body.value, revision: 1, locked: false },
        source: { scope: 'org', node: { id: ACME.research, kind: 'org', name: 'Research', redacted: false } } };
      await route.fulfill({ json: current });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const title = words.settingRegistry.agents.image_generation.title;
    await page.goto('/org/acme.research/settings/defaults');
    const row = page.locator('[data-setting="agents.image_generation"]');
    await expect(row).toContainText(title);
    await expect(row).toContainText(words.settingRegistry.agents.image_generation.description);
    const edit = row.getByRole('button', { name: words.settings.compact.editValue.replace('{{name}}', title), exact: true });
    await edit.click();
    const useValue = row.getByRole('checkbox', { name: words.settings.editor.nullableSet.replace('{{name}}', title), exact: true });
    await expect(useValue).not.toBeChecked();
    expect(writes).toHaveLength(0);
    await useValue.check();
    const choice = row.getByRole('combobox', { name: title, exact: true });
    await choice.click();
    await expect(page.getByRole('option', { name: words.choiceCatalog.imageGeneration.on.name, exact: true })).toBeVisible();
    await expect(page.getByRole('option', { name: words.choiceCatalog.imageGeneration.service.name, exact: true })).toBeVisible();
    await page.getByRole('option', { name: words.choiceCatalog.imageGeneration.off.name, exact: true }).click();
    // Leaving the compact editor commits its draft; there is no separate Save action.
    await edit.focus();
    await expect.poll(() => writes.length).toBe(1);
    expect(writes[0]).toMatchObject({ value: 'off', expected_revision: 0 });
    current = { ...current, writable: false, locked_by: { scope: 'org', node: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false } } };
    await page.reload();
    await expect(edit).toHaveCount(0);
    await expect(row.getByRole('button', { name: words.common.save, exact: true })).toHaveCount(0);
    expect(writes).toHaveLength(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // EN: Organization → Settings → Edit Image generation → Set Image generation → Off → leave the field; an inherited lock removes Edit.
    // 中文：组织 → 设置 → 编辑“图像生成” → 设置 → 关闭 → 离开字段；继承锁定后不显示编辑。
  });
}
