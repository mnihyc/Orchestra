/** Native settings failures retain the existing configuration entrance and revisioned rate controls. */
import { expect, test } from '@playwright/test';
import { askModels } from '../unit/ask-fixtures';
import { ACME, scenario, signIn } from './support';
import { fixture, language } from './ask-support';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1280, 400]) {
  test(`rate settings retry ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/usage?scope=node:${ACME.org}`);
    const open = page.getByRole('button', { name: words.usage.rates.open, exact: true }); await expect(open).toBeVisible();
    let failing = true;
    await page.route(`**/api/v1/nodes/${ACME.org}/settings`, async (route) => {
      if (route.request().method() === 'GET' && failing) await route.abort('failed'); else await route.continue();
    });
    // EN: Usage → See the rates → failed native settings read → disabled writes → Retry.
    // 中文：用量 → 查看费率 → 原生设置读取失败 → 禁用写入 → 重试。
    await open.click();
    const rates = page.getByRole('dialog').filter({ has: page.getByText(words.implicitAvailability.rateSettingsFailed, { exact: true }) });
    await expect(rates).toBeVisible();
    await expect(rates.getByRole('switch', { name: words.usage.rates.estimates, exact: true })).toBeDisabled();
    await info.attach(`rates-read-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    failing = false;
    await rates.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    await expect(page.getByRole('switch', { name: words.usage.rates.estimates, exact: true })).toBeEnabled();
  });

  test(`Ask configuration read retry ${locale}/${theme}/${width}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await fixture(page); const t = await language(page, request, locale, theme);
    await page.route((url) => url.pathname === '/api/v1/ask/models', (route) => route.fulfill({ json: askModels(ACME.org, { models: [], default_model: null, blocked_reason: 'empty', writable: true }) }));
    let failing = true;
    await page.route(`**/api/v1/nodes/${ACME.org}`, async (route) => {
      if (route.request().method() === 'GET' && failing) await route.abort('failed'); else await route.continue();
    });
    // EN: Topic → Ask → native blocked reason → retained Configure action → Retry → exact settings link.
    // 中文：话题 → 问一问 → 原生阻止原因 → 保留配置入口 → 重试 → 精确设置链接。
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const ask = page.locator('[data-view-window="ask"]');
    await expect(ask.getByRole('button', { name: t('ask.configureModels'), exact: true })).toHaveAttribute('aria-disabled', 'true');
    const retry = ask.getByRole('button', { name: t('common.tryAgain'), exact: true }); await expect(retry).toBeVisible();
    await info.attach(`Ask-configure-read-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    failing = false; await retry.click();
    await expect(ask.getByRole('link', { name: t('ask.configureModels'), exact: true })).toHaveAttribute('href', '/org/acme/settings/defaults?setting=ask.models');
  });
}
