/** One ordinary setting editor saves the shared search budgets shown to the agent and used by execution. */
import { expect, test } from '@playwright/test';
import type { ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn, supplyDefaults } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`shared search budgets save larger values ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const rows = [
      { name: 'search_query_chars', shipped: 1000, chosen: 2000, minimum: 1 },
      { name: 'search_results', shipped: 200, chosen: 201, minimum: 1 },
      { name: 'search_context', shipped: 5, chosen: 6, minimum: 0 },
    ] as const;
    const definitions: SettingDef[] = rows.map((row): SettingDef => ({
      key: `orch_tools.${row.name}`, title: english.settingRegistry.orch_tools[row.name].title,
      description: english.settingRegistry.orch_tools[row.name].description,
      scopes: ['org', 'thread'], group: 'agent', class: 'parameter', merge: 'replace',
      schema: { type: 'integer', minimum: row.minimum }, default: row.shipped,
      example: row.chosen, text_key: null, keywords: [],
    }));
    let current: ResolvedSetting[] = definitions.map((definition) => ({
      key: definition.key, value: definition.default, own: null, writable: true,
      locked_by: null, source: { scope: 'default', node: null }, parts: [],
    }));
    const writes: { key: string; value: number; expected_revision: number }[] = [];
    await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: definitions }));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: current }));
    await supplyDefaults(page, ACME.research, { registry: () => definitions, settings: () => current });
    for (const definition of definitions) {
      await page.route((url) => url.pathname === `/api/v1/nodes/${ACME.research}/settings/${definition.key}`, async (route) => {
        const before = current.find((value) => value.key === definition.key)!;
        const body = route.request().postDataJSON() as { value: number; expected_revision: number };
        expect(route.request().method()).toBe('PUT');
        expect(body.expected_revision).toBe(before.own?.revision ?? 0);
        writes.push({ key: definition.key, ...body });
        const saved = { ...before, value: body.value,
          own: { value: body.value, revision: body.expected_revision + 1, locked: false },
          source: { scope: 'org' as const, node: { id: ACME.research, kind: 'org' as const, name: 'Research', redacted: false } } };
        current = current.map((value) => value.key === definition.key ? saved : value);
        await route.fulfill({ json: saved });
      });
    }
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    // EN: Organization → Settings → Policies → Agent: inspect defaults, raise all three budgets and save.
    // 中文：组织 → 设置 → 策略 → 智能体：查看出厂值，调大三项预算并保存。
    await page.goto('/org/acme.research/settings/defaults');
    expect(writes).toHaveLength(0);
    for (const [index, row] of rows.entries()) {
      const setting = page.locator(`[data-setting="orch_tools.${row.name}"]`);
      await expect(setting).toContainText(words.settingRegistry.orch_tools[row.name].title);
      await expect(setting).toContainText(words.settingRegistry.orch_tools[row.name].description);
      await expect(setting.getByRole('spinbutton')).toHaveValue(String(row.shipped));
      await setting.getByRole('spinbutton').fill(String(row.chosen));
      await setting.getByRole('button', { name: words.common.save, exact: true }).click();
      await expect.poll(() => writes.length).toBe(index + 1);
      expect(writes[index]).toEqual({ key: `orch_tools.${row.name}`, value: row.chosen, expected_revision: 0 });
    }
    // EN: Reload and inspect the saved values; an inherited lock removes Save without resetting them.
    // 中文：刷新并核对已保存值；继承锁定后不显示“保存”，已保存的数值仍保留。
    await page.reload();
    for (const row of rows) await expect(page.locator(`[data-setting="orch_tools.${row.name}"]`).getByRole('spinbutton')).toHaveValue(String(row.chosen));
    current = current.map((value) => ({ ...value, writable: false,
      locked_by: { scope: 'org' as const, node: { id: ACME.org, kind: 'org' as const, name: 'Acme', redacted: false } } }));
    await page.reload();
    for (const row of rows) await expect(page.locator(`[data-setting="orch_tools.${row.name}"]`).getByRole('button', { name: words.common.save, exact: true })).toHaveCount(0);
    expect(writes).toHaveLength(3);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath(`shared-search-budgets-${locale}-${theme}-${width}.png`), fullPage: true });
  });
}
