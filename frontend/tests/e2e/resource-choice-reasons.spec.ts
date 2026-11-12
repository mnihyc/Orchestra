/** Readable unavailable resources stay in the real creation pickers with their native reasons. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { Template } from '../../src/generated/api-types';
import { signInScreenRoute } from '../../e2e/layout-fixtures';
import { id, KIND } from '../doubles/fixtures';
import { asAdmin } from './admin-mode-support';
import { DOUBLE, scenario } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440] as const) {
  test(`unavailable creation choices keep their reasons ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
    // EN: Template → Add environment → Template/Host → inspect disabled reasons → no creation.
    // 中文：模板 → 添加环境 → 模板／主机 → 查看禁用原因 → 确认未创建环境。
    await scenario();
    // The host inventory is also served by its native feed.
    const seeded = await request.post(`${DOUBLE}/__double/resources/template-hosts`, { data: { state: 'choice-reasons' } });
    expect(seeded.ok()).toBe(true);
    await signInScreenRoute(page, { locale, theme, width });
    // The read-only host must be checked with the ordinary session's permissions.
    await asAdmin(page, false);
    const words = locale === 'en' ? english : chinese;
    await page.route('**/api/v1/templates**', async (route) => {
      if (new URL(route.request().url()).pathname !== '/api/v1/templates') { await route.continue(); return; }
      const response = await route.fetch();
      const rows = await response.json() as Template[];
      const first = rows.find((row) => Object.hasOwn(row, 'driver')); if (first === undefined) throw new Error('The native fixture needs a readable template');
      await route.fulfill({ response, json: [...rows,
        { ...first, id: '00000000-0000-4000-8000-000000000081', name: 'Disabled recipe', enabled: false },
        { ...first, id: '00000000-0000-4000-8000-000000000082', name: 'Read-only recipe', access: { ...first.access, permissions: ['template.view'] } },
      ] });
    });
    const writes: string[] = [];
    page.on('request', (request) => { if (request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/environments') writes.push(request.url()); });
    await page.goto(`/resources/templates/${id(KIND.template, 1)}`);
    await page.getByRole('button', { name: words.environments.make.submit, exact: true }).click();
    const form = page.getByRole('dialog', { name: words.environments.make.title, exact: true });
    await form.getByRole('combobox', { name: words.environments.make.template, exact: true }).click();
    for (const [name, reason] of [['Disabled recipe', words.resourceChoices.disabled], ['Read-only recipe', words.resourceChoices.templateUse]] as const) {
      const option = page.getByRole('option', { name, exact: true });
      await expect(option).toHaveAttribute('aria-disabled', 'true'); await expect(option).toContainText(reason);
    }
    await page.keyboard.press('Escape');
    await form.getByRole('combobox', { name: words.environments.make.host, exact: true }).click();
    for (const [name, reason] of [['Closed host', words.resourceChoices.unschedulable], ['Other driver', words.resourceChoices.driver], ['Read-only host', words.resourceChoices.hostUse]] as const) {
      const option = page.getByRole('option', { name, exact: true });
      await expect(option).toHaveAttribute('aria-disabled', 'true'); await expect(option).toContainText(reason);
    }
    await page.screenshot({ path: info.outputPath('host-choice-reasons.png'), animations: 'disabled' });
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width + 1);
    expect(writes).toEqual([]);
    await page.keyboard.press('Escape');
    await form.getByRole('button', { name: words.common.cancel, exact: true }).click();
  });
}
