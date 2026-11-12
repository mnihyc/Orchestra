/** Thread -> Parameters -> Service is available without native search, and opening never opts in. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import { RUNS_ON, USUAL_MODEL } from '../doubles/fixtures';
import { begin, MAIN, showView } from './inspector-support';
import { signIn } from './support';

const parameter = {
  key: 'web_search', scopes: ['thread'], apply: 'new_session', title: 'Web search', type: 'enum', group: 'behaviour', capability: 'web_search', native_constraint: false,
  options: [{ value: 'disabled', label: 'Disabled' }, { value: 'service', label: 'Service' }], default: null,
};

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) for (const width of [400, 1440]) {
  test(`managed search on a model without native search ${locale}/${theme}/${width}`, async ({ page }, info) => {
    await begin('');
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
    const double = `http://127.0.0.1:${process.env.ORCH_DOUBLE_PORT ?? '41871'}/__double`;
    // The harness declaration and model facts feed native openings, repairs and parameter reads.
    expect((await page.request.post(`${double}/resources/report`, { data: { agent: RUNS_ON.codex, parameters: [parameter] } })).ok()).toBe(true);
    expect((await page.request.post(`${double}/model/entry`, { data: { model_name: USUAL_MODEL.name, parameters: {} } })).ok()).toBe(true);
    const writes: { value: Record<string, unknown>; expected_revision: number }[] = [];
    await page.route(`**/api/v1/nodes/${MAIN}/settings/agent.params`, async (route) => {
      if (route.request().method() === 'PUT') {
        // Interception makes the streamed body available before the native write is forwarded.
        const body: unknown = route.request().postDataJSON();
        if (typeof body !== 'object' || body === null || !('value' in body) || typeof body.value !== 'object' || body.value === null || Array.isArray(body.value)
          || !('expected_revision' in body) || typeof body.expected_revision !== 'number') throw new Error('The parameter write must carry its native body');
        writes.push({ value: body.value as Record<string, unknown>, expected_revision: body.expected_revision });
      }
      await route.continue();
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
    for (const mode of ['live', 'cached', 'indexed'] as const) {
      await expect(page.getByRole('option', { name: words.choiceCatalog.webSearch[mode].name, exact: true })).toHaveCount(0);
    }
    expect(writes).toEqual([]);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`managed-search-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await expect(picker).toBeFocused();
    await expect(picker).toHaveValue('');
    expect(writes).toEqual([]);
    // EN: Thread → Parameters → Web search: choose Service explicitly, verify its revision write, reload, then disable it.
    // 中文：线程 → 参数 → 网络搜索：明确选择“服务”，核对修订写入，刷新后选择“关闭”。
    for (const [index, mode] of (['service', 'disabled'] as const).entries()) {
      const written = page.waitForResponse((response) => response.request().method() === 'PUT'
        && new URL(response.url()).pathname === `/api/v1/nodes/${MAIN}/settings/agent.params`);
      await picker.click();
      await page.getByRole('option', { name: words.choiceCatalog.webSearch[mode].name, exact: true }).click();
      expect((await written).status()).toBe(200);
      await expect.poll(() => writes.length).toBe(index + 1);
      expect(writes[index]?.value.web_search).toBe(mode);
      expect(Number.isInteger(writes[index]?.expected_revision)).toBe(true);
      if (index > 0) expect(writes[index]?.expected_revision).toBe(writes[index - 1]!.expected_revision + 1);
      await expect(picker).toHaveValue(words.choiceCatalog.webSearch[mode].name);
      await page.reload();
      await showView(page, words.views.parameters.title);
      await expect(picker).toHaveValue(words.choiceCatalog.webSearch[mode].name);
      expect(writes).toHaveLength(index + 1);
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await info.attach(`managed-search-saved-${locale}-${theme}-${width}`, { body: await page.screenshot({ fullPage: true, animations: 'disabled' }), contentType: 'image/png' });
  });
}
