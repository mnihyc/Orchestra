/** The normal inherited settings row controls copies out, in both product languages. */
import { expect, test } from '@playwright/test';
import type { ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario, signIn } from './support';

const KEY = 'sharing.allow_copy_out';
for (const locale of ['en', 'zh-CN'] as const) {
  test(`copy-out policy uses the native boolean editor and inherited lock ${locale}`, async ({ page }) => {
    await scenario();
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    const title = words.settingRegistry.sharing.allow_copy_out.title;
    const definition: SettingDef = {
      key: KEY, title: 'Copies to other orgs', description: 'Whether things here may be duplicated or moved into another org.',
      group: 'collaboration', class: 'policy', scopes: ['system', 'org', 'project'], merge: 'replace',
      schema: { type: 'boolean' }, default: true, example: true, text_key: 'day100.builtin.sharing.allow_copy_out', keywords: [],
    };
    let current: ResolvedSetting = {
      key: KEY, value: true, own: null, source: { scope: 'default', node: null }, locked_by: null, writable: true, parts: [],
    };
    const writes: unknown[] = [];
    await page.route('**/api/v1/settings/registry', (route) => route.fulfill({ json: [definition] }));
    await page.route(`**/api/v1/nodes/${ACME.research}/settings`, (route) => route.fulfill({ json: [current] }));
    await page.route((url) => url.pathname === `/api/v1/nodes/${ACME.research}/settings/${KEY}`, async (route) => {
      expect(route.request().method()).toBe('PUT');
      const body = route.request().postDataJSON() as { value: boolean; expected_revision: number };
      writes.push(body);
      current = { ...current, value: body.value, own: { value: body.value, revision: 1, locked: false },
        source: { scope: 'org', node: { id: ACME.research, kind: 'org', name: 'Research', redacted: false } } };
      await route.fulfill({ json: current });
    });
    await page.goto(`/org/acme.research/settings/policies?setting=${KEY}`);
    const row = page.locator(`[data-setting="${KEY}"]`);
    await expect(row).toContainText(title);
    await expect(row).toContainText(words.settingRegistry.sharing.allow_copy_out.description);
    const control = row.getByRole('switch', { name: title, exact: true });
    await expect(control).toBeChecked();
    expect(writes).toEqual([]);
    await control.click();
    await expect.poll(() => writes).toEqual([{ value: false, expected_revision: 0 }]);
    await expect(control).not.toBeChecked();
    current = { ...current, writable: false, locked_by: {
      scope: 'org', node: { id: ACME.org, kind: 'org', name: 'Acme', redacted: false },
    } };
    await page.reload();
    await expect(row).toContainText(title);
    await expect(row.getByRole('switch')).toHaveCount(0);
    expect(writes).toHaveLength(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // EN: Organization → Settings → Policies → Copies to other orgs → Off; an inherited lock prevents edits.
    // 中文：组织 → 设置 → 策略 → 复制到其他组织 → 关闭；继承的锁定禁止编辑。
  });
}
