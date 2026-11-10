/** Start choices keep one actual origin while the environment picker opens and closes. */
import { expect, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { BindingOptions } from '../../src/generated/api-types';
import { RUNS_ON } from '../doubles/fixtures.ts';
import { requestsOf, setDefaults } from './start-support';
import { ACME, scenario, signIn } from './support';

for (const width of [1440, 400]) for (const theme of ['light', 'dark'] as const) for (const locale of ['en', 'zh-CN'] as const) {
  test(`one environment origin through the start picker ${width}/${theme}/${locale}`, async ({ page }, info) => {
    await scenario();
    await setDefaults(ACME.fixLogin, {
      'agent.default_provider_id': RUNS_ON.openAiKey,
      'agent.default_agent_id': RUNS_ON.codex,
      'agent.model': 'gpt-5.4',
      'agent.effort': 'high',
      'env.thread_default': 'shared',
    });
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ colorScheme: theme });
    await signIn(page);
    if (locale === 'zh-CN') {
      await page.goto('/settings/interface');
      await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
      await page.getByRole('option', { name: '简体中文', exact: true }).click();
    }
    await expect(page.locator('html')).toHaveAttribute('lang', locale);
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
    const words = locale === 'en' ? english : chinese;
    const origin = words.startImplicit.origin.shared_default;
    await page.goto(`/topic/${ACME.fixLogin}`);
    const starter = page.locator('[data-quick-start]');
    const choices = starter.locator('[data-runs-on]').getByRole('list');
    await expect(starter.getByText(words.start.topicDefault, { exact: true })).toHaveCount(1);
    for (const field of ['agent', 'provider', 'model', 'effort']) {
      const choice = choices.locator(`[data-start-value="${field}"]`);
      await expect(choice).toHaveAttribute('role', 'listitem');
      await expect(choice.getByText(words.start.topicDefault, { exact: true })).toHaveCount(0);
    }
    const value = starter.locator('[data-start-value="environment"]');
    await expect(value).toContainText('Workspace');
    await expect(value.getByText(origin, { exact: true })).toHaveCount(1);
    await starter.getByRole('button', { name: words.start.quick.change, exact: true }).click();
    const form = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
    const heading = form.getByRole('heading').filter({ hasText: words.start.where.title });
    await expect(heading.getByText(origin, { exact: true })).toHaveCount(1);
    const where = form.getByRole('combobox', { name: words.start.where.title, exact: true });
    await where.click();
    const workspace = page.getByRole('option', { name: 'Workspace', exact: true });
    await expect(workspace).toHaveAttribute('aria-selected', 'true');
    await expect(workspace.getByText(words.start.topicDefault, { exact: true })).toHaveCount(0);
    await expect(workspace).not.toHaveAccessibleDescription(new RegExp(words.start.topicDefault));
    await expect(heading.getByText(origin, { exact: true })).toHaveCount(1);
    await info.attach('selected-environment-origin', { body: await page.screenshot(), contentType: 'image/png' });
    await page.keyboard.press('Escape');
    await expect(form).toBeVisible();
    await expect(heading.getByText(origin, { exact: true })).toHaveCount(1);
    await expect(where).toHaveValue('Workspace');
    expect(await requestsOf('POST', /\/threads$/)).toHaveLength(0);
    const overflow = await form.evaluate((element) => element.scrollWidth - element.clientWidth);
    expect(overflow).toBeLessThanOrEqual(1);
    await form.getByRole('button', { name: words.common.cancel, exact: true }).click();
    await expect(form).toBeHidden();
    await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
      const response = await route.fetch();
      const payload: unknown = await response.json();
      const offered = payload as BindingOptions;
      await route.fulfill({ response, json: { ...offered,
        defaults: { provider_id: null, agent_id: null, model_name: null, effort: null, locked: false },
        environments: offered.environments.map((row) => ({ ...row, is_default: false })),
      } });
    });
    await page.reload();
    const ranked = page.locator('[data-quick-start] [data-runs-on]');
    const suggested = words.startImplicit.origin.suggested;
    await expect(ranked.getByText(suggested, { exact: true })).toHaveCount(1);
    const items = ranked.getByRole('listitem');
    expect(await items.count()).toBeGreaterThan(0);
    await expect(items.getByText(suggested, { exact: true })).toHaveCount(0);
    await expect(ranked.getByText(words.startFlow.separator.trim(), { exact: true })).toHaveCount(0);
    await info.attach('one-suggested-setup', { body: await page.screenshot({ path: info.outputPath('one-suggested-setup.png') }), contentType: 'image/png' });
    expect(await requestsOf('POST', /\/threads$/)).toHaveLength(0);
  });
}
