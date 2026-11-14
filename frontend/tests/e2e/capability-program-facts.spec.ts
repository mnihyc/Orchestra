/** EN: Thread → Parameters → read the selected program's capability facts → open the capability guide.
 * 中文：线程→参数→查看所选程序的能力事实→打开能力帮助。 */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import program from '../../../extensions/agents/claude-code/src/capability-program.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

for (const language of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [1440, 400]) for (const path of ['forwarded', 'separated'] as const) {
  test(`program capability facts ${path}/${language}/${theme}/${width}`, async ({ page }, info) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await begin('');
    const offered = path === 'forwarded' ? program.access.forwarded.facts : program.facts;
    const capabilities = ['generation', 'web_search', 'page_fetch'].map((capability) => ({
      capability, title: 'Capability', reason: null, program_default: true, provider: null, prices: {},
      route_view: { kind: 'native-program' }, facts: offered.filter((fact) => fact.capabilities.includes(capability)),
    }));
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
    if (language === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    const words = language === 'en' ? english : chinese;
    await page.goto(`/thread/${MAIN}`);
    await showView(page, words.views.parameters.title);
    const region = page.getByRole('region', { name: words.capabilityRoutes.title });
    for (const fact of offered) await expect(region.getByText(fact.text[language], { exact: true })).toBeVisible();
    const absent = path === 'forwarded' ? program.facts : program.access.forwarded.facts;
    for (const fact of absent) await expect(region.getByText(fact.text[language], { exact: true })).toHaveCount(0);
    expect(await region.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await info.attach(`program-facts-${path}-${language}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true }), contentType: 'image/png' });
    await region.getByRole('button', { name: words.capabilityRoutes.help }).click();
    await expect(page.getByRole('heading', { name: language === 'en' ? 'Provider web search' : '提供方网络搜索', exact: true })).toBeVisible();
  });
}
