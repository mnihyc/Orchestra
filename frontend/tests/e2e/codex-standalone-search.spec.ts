/** EN: Thread → Parameters → native endpoint modes → reload a service-only endpoint → help.
 * 中文：线程→参数→原生端点的搜索模式→刷新仅支持服务的端点→帮助。 */
import { expect, test } from '@playwright/test';
import type { ParameterView, ThreadParameters } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`declared standalone search choices ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    let supported = true;
    const withModes = (original: ThreadParameters): ThreadParameters => {
      const modes = supported ? ['disabled', 'cached', 'indexed', 'live', 'service'] : ['disabled', 'service'];
      const parameter: ParameterView = { key: 'web_search', value: supported ? 'indexed' : 'service', apply: 'new_session', source: { scope: 'thread', node: null }, locked_by: null, writable: true,
        def: { title: 'Web search', type: 'enum', options: { from_model: 'web_search' }, resolved_options: modes.map((value) => ({ value, label: value })), resolved_default: 'cached' } };
      return { ...original, parameters: [...original.parameters.filter((entry) => entry.key !== 'web_search'), parameter] };
    };
    const capabilities = () => [{ capability: 'web_search', title: 'Web search', program_default: false, reason: null, prices: {},
      route_view: { kind: supported ? 'native-model' : 'service' }, provider: { id: 'fixture-search', title: 'Team search' } }];
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as ThreadParameters;
      await route.fulfill({ response, json: { ...withModes(original), capabilities: capabilities() } });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening*`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { parameters: ThreadParameters | null };
      await route.fulfill({ response, json: { ...original, parameters: original.parameters === null ? null : { ...withModes(original.parameters), capabilities: capabilities() } } });
    });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
      await expect(page.locator('html')).toHaveAttribute('lang', locale);
    }
    const words = locale === 'en' ? english : chinese;
    await page.goto(`/thread/${MAIN}`);
    await showView(page, words.views.parameters.title);
    const picker = page.getByRole('combobox', { name: words.webSearch.title, exact: true });
    await expect(picker).toHaveValue(words.choiceCatalog.webSearch.indexed.name);
    await picker.click();
    for (const mode of ['cached', 'indexed', 'live', 'service'] as const) {
      await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch[mode].name, exact: true })).toBeVisible();
    }
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    const region = page.getByRole('region', { name: words.capabilityRoutes.title });
    await expect(region).toContainText(words.capabilityRoutes.routes['native-model']);
    supported = false;
    await page.reload();
    await showView(page, words.views.parameters.title);
    await expect(picker).toHaveValue(words.choiceCatalog.webSearch.service.name);
    await picker.click();
    await expect(page.getByRole('option')).toHaveCount(2);
    for (const mode of ['cached', 'indexed', 'live'] as const) {
      await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch[mode].name, exact: true })).toHaveCount(0);
    }
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    await expect(region).toContainText(words.capabilityRoutes.routes.service);
    await expect(region).not.toContainText(words.capabilityRoutes.routes['native-model']);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`standalone-search-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
    await region.getByRole('button', { name: words.capabilityRoutes.help }).click();
    await expect(page.getByRole('heading', { name: locale === 'en' ? 'Provider web search' : '提供方网络搜索', exact: true })).toBeVisible();
  });
}
