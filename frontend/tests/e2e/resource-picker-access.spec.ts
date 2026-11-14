/** Native access categories remain real groups inside the complete resource picker. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { BindingOptions } from '../../src/generated/api-types';
import { ACME, scenario } from './support';
import { signInScreenRoute } from '../../e2e/layout-fixtures';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`resource access groups ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await signInScreenRoute(page, { locale, theme, width });
    const words = locale === 'en' ? english : chinese;
    let usableName: string | undefined;
    await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
      const response = await route.fetch(); const offered = await response.json() as BindingOptions;
      const original = offered.environments[0];
      if (original === undefined) throw new Error('The access view fixture needs one native environment row.');
      usableName = original.environment.name;
      // Display-only alternatives: this case never submits a thread or claims a native permission effect.
      const environments = [
        // Keep this real identity's native name: its focused card and live row share that identity.
        { ...original, usable: true, reason: null, is_default: true, used: 'topic' as const },
        { ...original, environment: { ...original.environment, id: '00000000-0000-4000-8000-000000000092', name: 'Requestable environment' }, usable: false, reason: 'environment_use_required', is_default: false, used: 'topic' as const },
        { ...original, environment: { ...original.environment, id: '00000000-0000-4000-8000-000000000093', name: 'Blocked environment' }, usable: false, reason: 'disabled', is_default: false, used: 'topic' as const },
      ];
      const home = { id: '00000000-0000-4000-8000-000000000094', kind: 'project' as const, name: 'Regional team · shared source workspace', redacted: false };
      const otherHome = { ...home, id: '00000000-0000-4000-8000-000000000095', name: 'Other team · independent workspace' };
      const longRows = Array.from({ length: 24 }, (_, at) => ({ ...original,
        environment: { ...original.environment, id: `00000000-0000-4000-8000-${String(at + 100).padStart(12, '0')}`, name: `Regional environment ${at} · shared source workspace with a long directory name` },
        usable: true, reason: null, is_default: false, used: 'topic' as const,
      }));
      environments.push(...longRows);
      const grouped = environments.map((entry) => ({
        ...entry,
        home: entry.usable ? home : otherHome, home_path: [entry.usable ? home : otherHome],
        origin_machine: { id: '00000000-0000-4000-8000-000000000096', name: 'Shared host · regional source workspace machine' }, availability: [],
      }));
      await route.fulfill({ response, json: { ...offered, environments: grouped } });
    });
    await page.goto(`/topic/${ACME.fixLogin}`);
    const starter = page.locator('[data-quick-start]');
    await starter.getByRole('textbox').fill('Keep this first message');
    await starter.getByRole('button', { name: words.start.quick.change, exact: true }).click();
    const form = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
    const field = form.getByRole('combobox', { name: words.start.where.title, exact: true });
    await field.click();
    const canUse = page.getByRole('group', { name: words.start.where.canUse, exact: true });
    const ask = page.getByRole('group', { name: words.start.where.askAccess, exact: true });
    await expect(canUse).toBeVisible();
    if (usableName === undefined) throw new Error('The native usable environment was not read.');
    await expect(canUse.getByRole('option', { name: usableName, exact: true })).toBeEnabled();
    await expect(ask.getByRole('option', { name: 'Requestable environment', exact: true })).toBeEnabled();
    await expect(page.getByRole('option', { name: 'Blocked environment', exact: true })).toHaveAttribute('aria-disabled', 'true');
    await expect(ask.getByRole('option', { name: 'Blocked environment', exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`resource-access-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    // EN: New thread → Where → Can use / Ask for access, with a refused row kept outside the request group.
    // 中文：新建线程 → 运行位置 → 可以使用／申请使用，确认禁用选项未混入申请分组。
    await field.fill('Regional environment 17');
    await expect(page.getByRole('option', { name: /^Regional environment 17 /u })).toBeVisible();
    await expect(page.getByRole('option')).toHaveCount(1);
    await info.attach(`resource-filtered-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await form.getByRole('button', { name: words.common.cancel, exact: true }).click();
    await expect(starter.getByRole('textbox')).toHaveValue('Keep this first message');
  });
}
