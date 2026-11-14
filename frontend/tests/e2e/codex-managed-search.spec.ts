/** EN: Thread → Parameters → service choice without primary native search → Escape, preserving unset.
 * 中文：线程→参数→主模型不支持原生搜索时查看服务选项→按 Escape，保持未设置。 */
import { expect, test } from '@playwright/test';
import type { ParameterView, ThreadParameters } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

const parameter: ParameterView = {
  key: 'web_search', value: null, apply: 'new_session', source: { scope: 'default', node: null }, locked_by: null, writable: true,
  def: { title: 'Web search', type: 'enum', group: 'behaviour', capability: 'web_search', native_constraint: false,
    resolved_options: [{ value: 'disabled', label: 'Disabled' }, { value: 'service', label: 'Service' }], resolved_default: null },
};
const managed = (original: ThreadParameters): ThreadParameters => ({
  ...original,
  model: { ...original.model, options: original.model.options.map((option) => ({ ...option, parameters: {} })) },
  parameters: [...original.parameters.filter((entry) => entry.key !== 'web_search'), parameter],
});

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`Codex managed search keeps opening unpaid ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const writes: unknown[] = [];
    page.on('request', (request) => {
      if (request.method() === 'PUT' && new URL(request.url()).pathname === `/api/v1/nodes/${MAIN}/settings/agent.params`) writes.push(request.postDataJSON());
    });
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as ThreadParameters;
      await route.fulfill({ response, json: managed(original) });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening*`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { parameters: ThreadParameters | null };
      await route.fulfill({ response, json: { ...original, parameters: original.parameters === null ? null : managed(original.parameters) } });
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
    await expect(picker).toBeEnabled();
    await expect(picker).toHaveValue('');
    await picker.click();
    await expect(page.getByRole('option')).toHaveCount(2);
    await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch.service.name, exact: true })).toBeVisible();
    await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch.disabled.name, exact: true })).toBeVisible();
    for (const mode of ['cached', 'indexed', 'live'] as const) {
      await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch[mode].name, exact: true })).toHaveCount(0);
    }
    expect(writes).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`codex-managed-search-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    await expect(picker).toHaveValue('');
    expect(writes).toEqual([]);
  });
}
