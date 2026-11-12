/** A provider Use entry keeps its service list independently of its model list. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON } from '../doubles/fixtures.ts';
import { GRACE } from '../doubles/thread-scenes.ts';
import { DOUBLE, scenario, signIn } from './support.ts';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`provider model and service options remain separate ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario();
    const seeded = await fetch(`${DOUBLE}/__double/access/touch`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({
      as: 'ada', path: 'providers', id: RUNS_ON.openAiKey, change: { rows: [{ permission: 'provider.use', inherit: false,
        add: [{ holder: { kind: 'user', id: GRACE.id }, options: { models: ['@small'] } }] }] },
    }) });
    expect(seeded.ok).toBe(true);
    await signIn(page);
    await page.setViewportSize({ width, height: 900 });
    const appearance = await fetch(`${DOUBLE}/__double/first-steps/appearance`, { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ locale, theme }) });
    expect(appearance.ok).toBe(true);
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/resources/providers/${RUNS_ON.openAiKey}`);
    await page.getByRole('tab', { name: words.providers.tabs.settings, exact: true }).click();
    await page.getByRole('button', { name: words.actions.permissions.label, exact: true }).click();
    const access = page.getByRole('dialog').filter({ has: page.locator('[data-tutorial-target="permission-page"]') });
    await access.getByRole('button', { name: words.access.footer.byArea, exact: true }).click();
    const use = access.locator('[data-access-permission="provider.use"]');
    const permission = await use.getAttribute('aria-label');
    expect(permission).not.toBeNull();
    const menu = words.access.cell.menuOf.replace('{{permission}}', permission ?? '');
    const edit = words.access.cell.optionsOf.replace('{{name}}', 'Grace Hopper');
    const title = words.access.options.title.replace('{{holder}}', 'Grace Hopper').replace('{{permission}}', permission ?? '');
    await use.getByRole('button', { name: menu, exact: true }).click();
    await page.getByRole('menuitem', { name: edit, exact: true }).click();
    const entry = page.getByRole('dialog', { name: title, exact: true });
    await expect(entry.getByRole('textbox', { name: words.access.options.models, exact: true })).toHaveValue('@small');
    const services = entry.getByRole('textbox', { name: words.access.options.services, exact: true });
    await expect(services).toHaveValue('');
    await expect(entry.getByText(words.access.options.servicesHint, { exact: true })).toBeVisible();
    await services.fill('@web, image-*');
    await entry.getByRole('button', { name: words.access.options.save, exact: true }).click();
    await expect(entry).toHaveCount(0);
    await use.getByRole('button', { name: menu, exact: true }).click();
    await page.getByRole('menuitem', { name: edit, exact: true }).click();
    await expect(entry.getByRole('textbox', { name: words.access.options.services, exact: true })).toHaveValue('@web, image-*');
    await expect(entry.getByRole('textbox', { name: words.access.options.models, exact: true })).toHaveValue('@small');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await page.screenshot({ path: info.outputPath('provider-service-options.png') });
  });
}
