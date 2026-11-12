/** Own bilingual click path; view fixtures are separate from native carrier and policy controls. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { ACME, scenario } from './support';
import { prepareLayoutRoute, signInScreenRoute } from '../../e2e/layout-fixtures';
import { tunnel } from '../doubles/gateways';
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400] as const) {
  test(`gateway pairing evidence through the thread tool connection ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await signInScreenRoute(page, { locale, theme, width });
    await prepareLayoutRoute(page, { pattern: '/thread/$threadId', url: `/thread/${ACME.mainThread}`, fixture: 'tunnel-attached' });
    let rule: typeof tunnel.plan.decision.rule = 'lowest_combined_rtt';
    const path = `/api/v1/threads/${ACME.mainThread}/gateway`;
    await page.route(`**${path}`, async (route) => {
      if (route.request().method() !== 'GET') { await route.continue(); return; }
      const report = { ...tunnel, thread_id: ACME.mainThread,
        plan: { ...tunnel.plan, decision: { ...tunnel.plan.decision, rule,
          source_measured_at: rule === 'executor_samples_unavailable' ? null : tunnel.plan.decision.source_measured_at,
          combined_round_trip_ms: rule === 'lowest_combined_rtt' ? 40 : null } } };
      await route.fulfill({ json: report });
    });
    const words = (locale === 'en' ? english : chinese).regionalGateways;
    await page.goto(`/thread/${ACME.mainThread}`);
    // The report is in the thread header. An Environment sheet makes that
    // header inert on phones, and the opening already supplies an initial report.
    const refresh = () => page.getByLabel(words.toolConnection, { exact: true }).first().locator('..').getByRole('button', { name: words.refresh, exact: true }).click();
    await refresh();
    await expect(page.getByLabel(words.placement, { exact: true }).first()).toContainText(words.placementRule.lowest_combined_rtt);
    await expect(page.getByLabel(words.placement, { exact: true }).first()).toContainText(words.combinedRoundTrip.replace('{{value}}', '40'));
    for (const next of ['executor_pinned', 'executor_samples_unavailable'] as const) {
      rule = next;
      await refresh();
      await expect(page.getByLabel(words.placement, { exact: true }).first()).toContainText(words.placementRule[next]);
      await expect(page.getByLabel(words.placement, { exact: true }).first()).not.toContainText(words.combinedRoundTrip.replace('{{value}}', '0'));
    }
    await expect(page.getByLabel(words.placement, { exact: true }).first()).toContainText(words.notMeasured);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`gateway-pairing-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    // EN: Open a thread, refresh its tool connection, then read the choice, pin and missing-sample reasons.
    // 中文：打开线程并刷新工具连接，再核对网关选择、固定网关与缺少测量时的依据。
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400] as const) {
  test(`gateway recovery remains factual through refresh ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await scenario(); await signInScreenRoute(page, { locale, theme, width });
    await prepareLayoutRoute(page, { pattern: '/thread/$threadId', url: `/thread/${ACME.mainThread}`, fixture: 'tunnel-attached' });
    const shared = { id: 'us', label: 'US common', region: 'US', endpoints: [{ url: 'wss://common.example.test:9443', trust: 'pinned' as const }] };
    let state: 'attached' | 'unknown' = 'attached'; let recovered = false;
    await page.route(`**/api/v1/threads/${ACME.mainThread}/gateway`, async (route) => {
      await route.fulfill({ json: { ...tunnel, thread_id: ACME.mainThread, state,
        attached_gateway_id: state === 'unknown' ? null : recovered ? shared.id : 'asia',
        plan: { ...tunnel.plan, common_fallbacks: [...tunnel.plan.common_fallbacks, shared] } } });
    });
    const words = (locale === 'en' ? english : chinese).regionalGateways;
    await page.goto(`/thread/${ACME.mainThread}`);
    const refresh = () => page.getByLabel(words.toolConnection, { exact: true }).first().locator('..').getByRole('button', { name: words.refresh, exact: true }).click();
    await expect(page.getByLabel(words.toolConnection, { exact: true }).first()).toContainText('Asia tools');
    state = 'unknown';
    const unknownReply = page.waitForResponse((reply) => reply.request().method() === 'GET' && new URL(reply.url()).pathname === `/api/v1/threads/${ACME.mainThread}/gateway`);
    await refresh(); await unknownReply;
    const details = page.locator('details').filter({ has: page.locator('summary', { hasText: words.toolConnection }) }).first();
    await expect(details).toBeVisible();
    if (await details.getAttribute('open') === null) await details.locator('summary').click();
    await expect(page.getByLabel(words.toolConnection, { exact: true }).first()).toContainText(words.notReported);
    state = 'attached'; recovered = true;
    await refresh();
    const current = page.getByLabel(words.toolConnection, { exact: true }).first();
    await expect(current).toContainText('US common'); await expect(current).toContainText(words.fallback);
    await expect(page.getByLabel(words.placement, { exact: true }).first()).toContainText(words.plannedGateway);
    await expect(page.getByLabel(words.placement, { exact: true }).first()).toContainText('Asia tools');
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`gateway-recovered-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    // EN: Refresh the thread's unknown connection, then its recovered shared carrier and original plan.
    // 中文：在线程中刷新确认连接状态未知，再查看恢复后的共同网关与原定方案。
  });
}
