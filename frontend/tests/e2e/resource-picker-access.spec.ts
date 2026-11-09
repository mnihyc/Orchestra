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
    await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
      const response = await route.fetch(); const offered = await response.json() as BindingOptions;
      const original = offered.environments[0];
      if (original === undefined) throw new Error('The access view fixture needs one native environment row.');
      // Display-only alternatives: this case never submits a thread or claims a native permission effect.
      const environments = [
        { ...original, environment: { ...original.environment, name: 'Usable environment' }, usable: true, reason: null, is_default: true, used: 'topic' as const },
        { ...original, environment: { ...original.environment, id: '00000000-0000-4000-8000-000000000092', name: 'Requestable environment' }, usable: false, reason: 'environment_use_required', is_default: false, used: 'topic' as const },
        { ...original, environment: { ...original.environment, id: '00000000-0000-4000-8000-000000000093', name: 'Blocked environment' }, usable: false, reason: 'disabled', is_default: false, used: 'topic' as const },
      ];
      await route.fulfill({ response, json: { ...offered, environments } });
    });
    await page.goto(`/topic/${ACME.fixLogin}`);
    const starter = page.locator('[data-quick-start]');
    await starter.getByRole('textbox').fill('Keep this first message');
    await starter.getByRole('button', { name: words.start.quick.change, exact: true }).click();
    const form = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
    await form.getByRole('combobox', { name: words.start.where.title, exact: true }).click();
    const canUse = page.getByRole('group', { name: words.start.where.canUse, exact: true });
    const ask = page.getByRole('group', { name: words.start.where.askAccess, exact: true });
    await expect(canUse.getByRole('option', { name: 'Usable environment', exact: true })).toBeEnabled();
    await expect(ask.getByRole('option', { name: 'Requestable environment', exact: true })).toBeEnabled();
    await expect(page.getByRole('option', { name: 'Blocked environment', exact: true })).toHaveAttribute('aria-disabled', 'true');
    await expect(ask.getByRole('option', { name: 'Blocked environment', exact: true })).toHaveCount(0);
    await info.attach(`resource-access-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    // EN: New thread → Where → Can use / Ask for access, with a refused row kept outside the request group.
    // 中文：新建线程 → 运行位置 → 可以使用／申请使用，确认禁用选项未混入申请分组。
    await page.keyboard.press('Escape');
    await form.getByRole('button', { name: words.common.cancel, exact: true }).click();
    await expect(starter.getByRole('textbox')).toHaveValue('Keep this first message');
  });
}
