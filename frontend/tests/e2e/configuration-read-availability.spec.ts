/** Native settings failures retain the existing configuration entrance and revisioned rate controls. */
import { expect, test } from '@playwright/test';
import { askModels } from '../unit/ask-fixtures';
import type { MeBootstrapReply, OrgTreeGetReply } from '../../src/generated/api-types';
import { ACME, scenario, signIn } from './support';
import { fixture, language, openAsk } from './ask-support';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1280, 400]) {
  test(`rate settings retry ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    let failing = true;
    // The tree can seed the same settings cache that the rate dialog reads.
    await page.route((url) => url.pathname === `/api/v1/orgs/${ACME.org}/tree`, async (route) => {
      const response = await route.fetch();
      const data = await response.json() as OrgTreeGetReply;
      await route.fulfill({ response, json: { ...data, settings: null } });
    });
    await page.route(`**/api/v1/nodes/${ACME.org}/settings`, async (route) => {
      if (route.request().method() === 'GET' && failing) await route.abort('failed'); else await route.continue();
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/usage?scope=node:${ACME.org}`);
    const open = page.getByRole('button', { name: words.usage.rates.open, exact: true }); await expect(open).toBeVisible();
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
    await fixture(page);
    // A root outside the shell's ordinary org list needs its own native configuration-name read.
    await page.route('**/api/v1/me/bootstrap', async (route) => {
      const response = await route.fetch();
      const data = await response.json() as MeBootstrapReply;
      data.orgs = data.orgs.filter((org) => org.id !== ACME.org);
      await route.fulfill({ response, json: data });
    });
    await page.route((url) => url.pathname === '/api/v1/ask/models', (route) => route.fulfill({ json: askModels(ACME.org, { models: [], default_model: null, blocked_reason: 'empty', writable: true }) }));
    let failing = true;
    await page.route(`**/api/v1/nodes/${ACME.org}`, async (route) => {
      if (route.request().method() === 'GET' && failing) await route.abort('failed'); else await route.continue();
    });
    const t = await language(page, request, locale, theme);
    // EN: Topic → Ask → native blocked reason → retained Configure action → Retry → exact settings link.
    // 中文：话题 → 问一问 → 原生阻止原因 → 保留配置入口 → 重试 → 精确设置链接。
    await page.goto(`/topic/${ACME.fixLogin}`);
    await expect(page.getByRole('heading', { name: 'Fix login', exact: true })).toBeVisible();
    await openAsk(page, t);
    const ask = page.locator('[data-view-window="ask"]');
    await expect(ask.getByRole('button', { name: t('ask.configureModels'), exact: true })).toHaveAttribute('aria-disabled', 'true');
    const retry = ask.getByRole('button', { name: t('common.tryAgain'), exact: true }); await expect(retry).toBeVisible();
    await info.attach(`Ask-configure-read-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    failing = false; await retry.click();
    await expect(ask.getByRole('link', { name: t('ask.configureModels'), exact: true })).toHaveAttribute('href', '/org/acme/settings/defaults?setting=ask.models');
  });
}
