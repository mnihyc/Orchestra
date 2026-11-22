/** Thread -> Parameters -> the indexed native choice, from the server's resolved list. */
import { expect, test } from '@playwright/test';
import type { ParameterView, ThreadParameters } from '../../src/generated/api-types';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

const parameter: ParameterView = { key: 'web_search', value: null, apply: 'new_session', source: { scope: 'default', node: null }, locked_by: null, writable: true,
  def: { title: 'Web search', type: 'enum', options: { from_model: 'web_search' }, resolved_options: [{ value: 'disabled', label: 'Disabled' }, { value: 'indexed', label: 'indexed' }], resolved_default: 'disabled' } };
const withMode = (original: ThreadParameters): ThreadParameters => ({ ...original, parameters: [...original.parameters.filter((entry) => entry.key !== 'web_search'), parameter] });
for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`indexed mode remains an explicit native choice ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    await page.route(`**/api/v1/threads/${MAIN}/parameters`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as ThreadParameters;
      await route.fulfill({ response, json: withMode(original) });
    });
    await page.route(`**/api/v1/threads/${MAIN}/opening*`, async (route) => {
      const response = await route.fetch();
      const original = await response.json() as { parameters: ThreadParameters | null };
      await route.fulfill({ response, json: { ...original, parameters: original.parameters === null ? null : withMode(original.parameters) } });
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
    await expect(picker).toHaveValue(words.choiceCatalog.webSearch.disabled.name);
    await picker.click();
    await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch.indexed.name, exact: true })).toBeVisible();
    await expect(page.getByRole('option')).toHaveCount(2);
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    await expect(picker).toHaveValue(words.choiceCatalog.webSearch.disabled.name);
    await info.attach('indexed-mode-choice', { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
