/** Normalized ACP search state; the manifest/descriptor cases cover its shared-resolver source. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`ACP search default click path ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await begin('');
    const capabilities = [{
      capability: 'web_search', title: 'Web search', setting: 'agent.params.web_search',
      mode: 'disabled', modes: ['disabled'], program_default: true,
      route_view: null, reason: 'disabled', detail: null,
      provider: null, offering: null, prices: {}, item: 'web_search', ledger_unit: 'search',
    }];
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as Record<string, unknown>;
      await route.fulfill({ response, json: { ...original, capabilities } });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening?*`, async (route) => {
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
    const search = region.getByRole('listitem').filter({ hasText: words.capabilityRoutes.names.web_search });
    await expect(search).toHaveCount(1);
    await expect(search).toContainText(words.capabilityRoutes.reasons.disabled);
    await expect(search).not.toContainText(words.capabilityRoutes.routes.service);
    await expect(search.getByRole('combobox')).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`acp-capability-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    const guide = region.getByRole('button', { name: words.capabilityRoutes.help });
    await guide.focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('heading', { name: locale === 'en' ? 'Provider web search' : '提供方网络搜索', exact: true })).toBeVisible();
    // EN: Open a thread → Parameters → Capabilities → Web search is Off → open How capabilities run.
    // 中文：打开线程 → “参数” → “能力” → “网页搜索”为“关闭” → 打开“能力如何运行”。
  });
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) {
  test(`ACP shared search choice save and unavailable route ${locale}/${theme}/${width}`, async ({ page }) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await begin('');
    let selected = 'disabled';
    let backend = true;
    let locked = false;
    const writes: Record<string, unknown>[] = [];
    const project = (original: Record<string, unknown>) => ({
      ...original,
      parameters: [{
        key: 'web_search', value: selected, source: { scope: 'thread', node: null },
        locked_by: locked ? { scope: 'org', node: null } : null, writable: !locked, apply: 'new_session',
        def: {
          capability: 'web_search', title: 'Web search', group: 'behaviour', type: 'enum',
          resolved_default: 'disabled', native_constraint: { enum: ['disabled', 'live'] },
          resolved_options: (backend ? ['disabled', 'live', 'service'] : ['disabled', 'live']).map((value) => ({ value, label: value })),
        },
      }],
      capabilities: [{
        capability: 'web_search', title: 'Web search', setting: 'agent.params.web_search', mode: selected,
        modes: backend ? ['disabled', 'live', 'service'] : ['disabled', 'live'], program_default: false,
        route_view: null, reason: selected === 'disabled' ? 'disabled' : backend ? null : 'backend_not_configured',
        detail: null, provider: null, offering: null, prices: {}, item: 'web_search', ledger_unit: 'search',
      }],
    });
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      await route.fulfill({ response, json: project(await response.json() as Record<string, unknown>) });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening?*`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { parameters: Record<string, unknown> | null };
      await route.fulfill({ response, json: { ...original, parameters: original.parameters === null ? null : project(original.parameters) } });
    });
    await page.route(`**/api/v1/nodes/${MAIN}/settings/agent.params`, async (route) => {
      if (route.request().method() !== 'PUT') return route.continue();
      const body = route.request().postDataJSON() as { value: Record<string, unknown>; expected_revision: number };
      expect(typeof body.expected_revision).toBe('number');
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      selected = String(body.value.web_search);
      writes.push(body);
      await route.fulfill({ response });
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
    const row = page.getByTestId('parameter-row').filter({ hasText: words.webSearch.title });
    const choice = row.getByRole('combobox', { name: words.webSearch.title, exact: true });
    await choice.click();
    await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch.live.name, exact: true })).toBeVisible();
    await page.getByRole('option', { name: words.choiceCatalog.webSearch.service.name, exact: true }).click();
    await expect(row.getByRole('status')).toHaveText(words.inspector.parameters.saved);
    expect(writes).toHaveLength(1);
    expect(writes[0]).toMatchObject({ value: { web_search: 'service' } });
    backend = false;
    await page.reload();
    await showView(page, words.views.parameters.title);
    await choice.click();
    await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch.service.name, exact: true })).toHaveCount(0);
    await page.keyboard.press('Escape');
    expect(selected).toBe('service');
    expect(writes).toHaveLength(1);
    locked = true;
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(choice).toBeDisabled();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    // EN: Thread → Parameters → Web search → Service → Saved; a lost backend removes Service from choices; a place lock disables the control.
    // 中文：线程 → “参数” → “网络搜索” → “服务” → “已保存”；后端不可用后选项不再列出“服务”，上级锁定时控件禁用。
  });
}
