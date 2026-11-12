/** EN: Provider → Settings → Services → edit → save → reopen; navigation preserves the saved choice.
 * 中文：提供方 → 设置 → 服务 → 编辑 → 保存 → 重新打开；页面导航后保留已保存的选项。 */
import { expect, test, type Request } from '@playwright/test';
import type { Provider, ProviderUpdateBody } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures.ts';
import { bodySent, DOUBLE, scenario, signIn } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`declared service opt-in uses the provider click path ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    expect((await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }) })).ok).toBe(true);
    const offering: NonNullable<Provider['service_catalogue']>[number] = {
      id: 'fixture-search', title: 'Fixture Search', capabilities: ['web_search'], available: true, enabled: false,
      groups: ['web'], declared_groups: ['web'], units: ['search'], catalogue_prices: { search: { amount: '0.005', currency: 'USD', unit: 'search' } },
      prices: { search: { amount: '0.005', currency: 'USD', unit: 'search' } }, explicit_prices: [],
    };
    expect((await fetch(`${DOUBLE}/__double/resources/provider-services`, { method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ id: RUNS_ON.openAiKey, catalogue: [offering] }) })).ok).toBe(true);
    const writes: Request[] = [];
    page.on('request', (request) => {
      if (request.method() === 'PATCH' && new URL(request.url()).pathname === `/api/v1/providers/${RUNS_ON.openAiKey}`) writes.push(request);
    });
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}`);
    await page.getByRole('tab', { name: words.providers.tabs.settings, exact: true }).click();
    const row = page.locator('[data-provider-service="fixture-search"]');
    await expect(row).toContainText(words.providerServices.disabled);
    await expect(row).toContainText('0.005 USD');
    expect(writes).toHaveLength(0);
    await row.getByRole('button', { name: words.providerServices.edit, exact: true }).click();
    const dialog = page.getByRole('dialog', { name: words.providerServices.edit, exact: true });
    await dialog.getByRole('checkbox', { name: words.providerServices.enable, exact: true }).check();
    await dialog.getByRole('checkbox', { name: words.providerServices.inheritGroups, exact: true }).uncheck();
    await dialog.getByRole('textbox', { name: words.providerServices.groups, exact: true }).fill('web, research');
    await dialog.getByRole('combobox', { name: words.providerServices.price.replace('{{unit}}', words.providerServices.units.search), exact: true }).click();
    await page.getByRole('option', { name: words.providerServices.creditsPrice, exact: true }).click();
    const credits = dialog.getByRole('textbox', { name: words.providerServices.credits.replace('{{unit}}', words.providerServices.units.search), exact: true });
    await credits.fill('0.000750');
    await dialog.getByRole('button', { name: words.resources.edit.save, exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(writes).toHaveLength(1);
    const write = writes[0];
    if (write === undefined) throw new Error('Saving the service must send its native provider update');
    const written = await bodySent(write) as ProviderUpdateBody;
    expect(written.service_offerings?.['fixture-search']).toMatchObject({ enabled: true, groups: ['web', 'research'], prices: { search: { amount: '0.000750', currency: 'credits', unit: 'search' } } });
    await page.reload();
    await row.getByRole('button', { name: words.providerServices.edit, exact: true }).click();
    await expect(dialog.getByRole('checkbox', { name: words.providerServices.enable, exact: true })).toBeChecked();
    await expect(dialog.getByRole('textbox', { name: words.providerServices.groups, exact: true })).toHaveValue('web, research');
    await expect(credits).toHaveValue('0.000750');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('provider-service-editor.png') });
    await dialog.getByRole('button', { name: words.common.cancel, exact: true }).click();
    await expect(row).toContainText(words.providerServices.enabled);
    await expect(row).toContainText(words.providerServices.currencyCredits);
    await page.getByRole('button', { name: words.providerServices.guide, exact: true }).click();
    await expect(page.getByRole('heading', { name: words.providerServices.guide, exact: true })).toBeVisible();
    expect(writes).toHaveLength(1);
  });
}
