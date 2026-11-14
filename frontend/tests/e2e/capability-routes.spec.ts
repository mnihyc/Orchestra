/** Own click path: thread → Parameters → capabilities → help, in English and Chinese. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`capability route click path ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await begin('');
    const base = { title: 'Fixture capability', reason: null, program_default: false, provider: null, prices: {} };
    const capabilities = [
      { ...base, capability: 'generation', program_default: true, route_view: { kind: 'native-program' } },
      { ...base, capability: 'web_search', route_view: { kind: 'service' }, provider: { id: 'fixture-search', title: 'Team search' }, prices: { search: { amount: '0.00075', currency: 'USD', unit: 'search' } } },
      { ...base, capability: 'image_generation', route_view: null, reason: 'current_authority_refused' },
      { ...base, capability: 'page_fetch', route_view: null, reason: 'disabled' },
    ];
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as Record<string, unknown>;
      await route.fulfill({ response, json: { ...original, capabilities } });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening*`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { parameters: Record<string, unknown> | null };
      await route.fulfill({ response, json: { ...original, parameters: original.parameters === null ? null : { ...original.parameters, capabilities } } });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${MAIN}`);
    await showView(page, words.views.parameters.title);
    const region = page.getByRole('region', { name: words.capabilityRoutes.title });
    await expect(region).toBeVisible();
    await expect(region.locator('[data-capability="web_search"]')).toContainText(words.capabilityRoutes.names.web_search);
    await expect(region).toContainText(words.capabilityRoutes.routes['native-program']);
    await expect(region).toContainText(words.capabilityRoutes.programDefault);
    await expect(region).toContainText(words.capabilityRoutes.routes.service);
    await expect(region).toContainText('Team search');
    await expect(region).toContainText('0.00075');
    await expect(region).toContainText(words.capabilityRoutes.reasons.current_authority_refused);
    await expect(region).toContainText(words.capabilityRoutes.reasons.disabled);
    await expect(region.getByRole('combobox')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`capability-routes-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    // EN: Reopen Parameters after the endpoint reports native support; its original-unit quote is retained.
    // 中文：端点报告原生支持后，刷新并重新打开“参数”，核对原始货币报价。
    capabilities[1]!.route_view = { kind: 'native-model' };
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(region).toContainText(words.capabilityRoutes.routes['native-model']);
    await expect(region).toContainText('0.00075');
    await expect(region).toContainText('USD');
    // EN: An endpoint with a provider-dependent engine reports native support without inventing a fixed quote.
    // 中文：搜索引擎由提供方决定时，保留原生支持，并明确价格未提供，不猜测固定费用。
    capabilities[1]!.provider = { id: 'fixture-router', title: 'Router account' };
    capabilities[1]!.prices = {};
    await page.reload();
    await showView(page, words.views.parameters.title);
    const router = region.locator('[data-capability="web_search"]');
    await expect(router).toContainText(words.capabilityRoutes.routes['native-model']);
    await expect(router).toContainText('Router account');
    await expect(router).toContainText(words.capabilityRoutes.noPrice);
    await expect(router).not.toContainText('0.00075');
    await info.attach(`capability-router-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    // EN: The same endpoint then refuses the tool; the reason replaces the route and stale price.
    // 中文：同一端点随后不再提供该工具；刷新后应显示原因，并移除旧执行方式及价格。
    capabilities[1]!.route_view = null;
    capabilities[1]!.reason = 'native_tool_not_declared';
    capabilities[1]!.prices = {};
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(region).toContainText(words.capabilityRoutes.reasons.native_tool_not_declared);
    await expect(region).not.toContainText('0.00075');
    await expect(region).not.toContainText(words.capabilityRoutes.routes['native-model']);
    // EN: A forwarded program can declare its own page reader; read that route in the same inspector.
    // 中文：转发模式下的程序可声明自带的网页读取工具；在同一检查器中核对该方式。
    capabilities[3]!.route_view = { kind: 'native-program' };
    capabilities[3]!.reason = null;
    capabilities[2]!.reason = 'program_support_not_applicable';
    capabilities.push({ ...base, capability:'image_input', route_view:null, reason:'offering_does_not_enable_mode' });
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(region.locator('[data-capability="page_fetch"]')).toContainText(words.capabilityRoutes.routes['native-program']);
    await expect(region.locator('[data-capability="page_fetch"]')).not.toContainText(words.capabilityRoutes.reasons.disabled);
    // EN: A missing carried model tool or explicitly text-only model never becomes a program fallback.
    // 中文：模型工具缺失或模型明确仅支持文本时，不应改称程序自带能力。
    await expect(region.locator('[data-capability="web_search"]')).not.toContainText(words.capabilityRoutes.routes['native-program']);
    await expect(region.locator('[data-capability="image_input"]')).toContainText(words.capabilityRoutes.reasons.offering_does_not_enable_mode);
    await expect(region.locator('[data-capability="image_input"]')).not.toContainText(words.capabilityRoutes.routes['native-program']);
    await expect(region.locator('[data-capability="image_generation"]')).toContainText(words.capabilityRoutes.reasons.program_support_not_applicable);
    // EN: A currently usable service with no admitted provider Read shows its route, with names and prices withheld.
    // 中文：服务当前可用但本次未获提供方读取权限时，显示执行方式，不披露名称及价格。
    capabilities[1]!.route_view = { kind: 'service' };
    capabilities[1]!.reason = null;
    capabilities[1]!.provider = null;
    capabilities[1]!.prices = {};
    await page.reload();
    await showView(page, words.views.parameters.title);
    const paid = region.locator('[data-capability="web_search"]');
    await expect(paid).toContainText(words.capabilityRoutes.routes.service);
    await expect(paid).toContainText(words.capabilityRoutes.noPrice);
    await expect(paid).not.toContainText('Team search');
    await expect(paid).not.toContainText('Router account');
    // EN: After current Use/policy closes, reopen Parameters; unavailable replaces the service without another provider.
    // 中文：当前使用权限或策略关闭后，重新打开“参数”；显示不可用，不自动改用其他提供方。
    capabilities[1]!.route_view = null;
    capabilities[1]!.reason = 'current_authority_refused';
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(paid).toContainText(words.capabilityRoutes.reasons.current_authority_refused);
    await expect(paid).not.toContainText(words.capabilityRoutes.routes.service);
    const guide = region.getByRole('button', { name: words.capabilityRoutes.help });
    await guide.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: locale === 'en' ? 'Provider web search' : '提供方网络搜索', exact: true })).toBeVisible();
    // EN: Open the thread, choose Parameters, read the route/provider/price/reason, then open its guide.
    // 中文：打开线程，选择“参数”，查看能力的方式、提供方、价格和原因，再打开帮助。
  });
}
