/** Existing sharing summaries and approval-rule inspectors expose native read failures and Retry. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { MAIN } from './change-support';
import { scenario, signIn } from './support';
import { showWorkbenchView } from './workbench-support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1280, 400]) {
  test(`sharing and rule read availability ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme }); await signIn(page);
    expect((await page.request.post(`http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double/request-feeds/fixtures`)).ok()).toBe(true);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    let defaultsFail = true;
    await page.route(/\/api\/v1\/nodes\/[^/?]+\/defaults(?:\?.*)?$/, async (route) => {
      if (route.request().method() === 'GET' && defaultsFail) await route.abort('failed'); else await route.continue();
    });
    // EN: Templates → actual home sharing summary → failed read → Retry → current policy.
    // 中文：模板 → 实际归属地的共享摘要 → 读取失败 → 重试 → 当前规则。
    await page.goto('/resources/templates');
    const group = page.locator('[data-resource-group]').filter({ has: page.getByText(words.implicitAvailability.sharingFailed, { exact: true }) }).first();
    await expect(group).toBeVisible();
    await info.attach(`sharing-read-${locale}-${theme}-${width}`, { body: await page.screenshot(), contentType: 'image/png' });
    defaultsFail = false;
    await group.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    await expect(page.getByText(words.implicitAvailability.sharingFailed, { exact: true })).toHaveCount(0);
    let rulesFail = true;
    await page.route(/\/api\/v1\/threads\/[^/?]+\/approval-rules(?:\?.*)?$/, async (route) => {
      if (route.request().method() === 'GET' && rulesFail) await route.abort('failed'); else await route.continue();
    });
    // EN: Thread → Parameters → approval rules → failed read → Retry → native rules or empty list.
    // 中文：线程 → 参数 → 审批规则 → 读取失败 → 重试 → 原生规则或空列表。
    await page.goto(`/thread/${MAIN}`); await showWorkbenchView(page, 'Parameters');
    const rules = page.locator('[data-approval-rules]');
    await expect(rules.getByText(words.approvals.rules.unavailable, { exact: true })).toBeVisible();
    rulesFail = false;
    await rules.getByRole('button', { name: words.common.tryAgain, exact: true }).click();
    await expect(rules.getByText(words.approvals.rules.unavailable, { exact: true })).toHaveCount(0);
    await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  });
}
