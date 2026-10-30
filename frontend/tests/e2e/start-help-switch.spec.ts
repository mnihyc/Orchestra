/** Start, help and switch controls at both widths, languages and themes. */
import { expect, type Page, test } from '@playwright/test';
import english from '../../src/i18n/en.json' with { type: 'json' };
import chinese from '../../src/i18n/zh-CN.json' with { type: 'json' };
import type { BindingOptions } from '../../src/generated/api-types';
import { expectOrdinaryWizardControls, expectSameActionRow } from './change-control-geometry';
import { MAIN, openHistory } from './change-support';
import { requestsOf } from './start-support';
import { bodySent, ACME, scenario, signIn } from './support';

async function language(page: Page, locale: 'en' | 'zh-CN'): Promise<void> {
  if (locale === 'zh-CN') {
    await page.goto('/settings/interface');
    await page.getByRole('combobox', { name: /^(Language|语言)$/ }).click();
    await page.getByRole('option', { name: '简体中文', exact: true }).click();
  }
  await expect(page.locator('html')).toHaveAttribute('lang', locale);
}

for (const width of [1440, 400]) for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test.describe(`start/help/switch ${width}/${locale}/${theme}`, () => {
    test.beforeEach(async ({ page }) => {
      await scenario();
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ colorScheme: theme });
    });

    test('four choices, optional setup and one help view retain the original draft', async ({ page }, info) => {
      await signIn(page); await language(page, locale);
      const words = locale === 'en' ? english : chinese;
      await page.goto(`/topic/${ACME.fixLogin}`);
      const starter = page.locator('[data-quick-start]');
      await starter.getByRole('textbox').fill('Keep this first message');
      await starter.getByRole('button', { name: words.start.quick.change, exact: true }).click();
      const form = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
      await expect(form).toBeVisible();
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme);
      await expect(form.getByRole('combobox')).toHaveCount(4);
      await expect(form.getByRole('textbox')).toHaveCount(0);
      await expect(form.getByRole('radiogroup')).toHaveCount(0);
      await expectOrdinaryWizardControls(form, 'four initial choices');
      await expectSameActionRow(form.locator('[data-start-actions]'));
      await info.attach('start-four-choices', { body: await page.screenshot(), contentType: 'image/png' });

      await form.getByRole('button', { name: words.start.options.title, exact: true }).click();
      const name = form.getByRole('textbox', { name: new RegExp(`^${words.start.options.name}`) });
      await name.fill('Retained setup');
      await expect(form.getByRole('textbox', { name: words.start.message.title, exact: true })).toHaveValue('Keep this first message');
      await page.keyboard.press('F1');
      const help = page.locator('[data-help-reading]');
      await expect(help).toBeVisible();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect(page.getByRole('alertdialog')).toHaveCount(0);
      await expect(page).toHaveURL(new RegExp(`/topic/${ACME.fixLogin}$`));
      await help.getByRole('tab', { name: new RegExp(`^${words.help.parts.articles}`) }).click();
      await help.getByRole('button', { name: words.craftGuides.gettingStarted, exact: true }).click();
      const article = help.getByRole('region', { name: words.craftGuides.gettingStarted, exact: true });
      await expect(article).toBeVisible();
      const position = await article.evaluate((element) => { element.scrollTop = Math.min(160, element.scrollHeight - element.clientHeight); element.focus(); return element.scrollTop; });
      await page.keyboard.press('Control+k');
      const finder = page.getByRole('dialog', { name: words.finder.title, exact: true });
      await expect(finder).toBeVisible();
      await expect(help).toBeHidden();
      await page.keyboard.press('Escape');
      await expect(finder).toBeHidden();
      await expect(article).toBeVisible();
      expect(await article.evaluate((element) => element.scrollTop)).toBe(position);
      await info.attach('help-reading-view', { body: await page.screenshot(), contentType: 'image/png' });
      await help.getByRole('button', { name: words.helpFlow.back, exact: true }).click();
      await expect(form).toBeVisible();
      await expect(name).toHaveValue('Retained setup');
      await expect(form.getByRole('textbox', { name: words.start.message.title, exact: true })).toHaveValue('Keep this first message');
      await expectOrdinaryWizardControls(form, 'returned setup');
      await expectSameActionRow(form.locator('[data-start-actions]'));
      await form.getByRole('button', { name: words.common.cancel, exact: true }).click();
      await expect(starter.getByRole('textbox')).toHaveValue('Keep this first message');
      expect(await requestsOf('POST', /\/threads$/)).toHaveLength(0);
    });

    test('an impossible new-environment setup is disabled before any creation request', async ({ page }, info) => {
      await signIn(page); await language(page, locale);
      const words = locale === 'en' ? english : chinese;
      await page.route(`**/nodes/${ACME.fixLogin}/binding/options**`, async (route) => {
        if (new URL(route.request().url()).searchParams.get('environment_new') !== 'true') { await route.continue(); return; }
        const response = await route.fetch();
        const payload: unknown = await response.json();
        const offered = payload as BindingOptions;
        await route.fulfill({ response, json: { ...offered, providers: offered.providers.map((provider) => ({ ...provider,
          agents: provider.agents.map((agent) => ({ ...agent, usable: false, reason: 'executor_missing',
            arrangements: agent.arrangements.map((arrangement) => ({ ...arrangement, available: false, reason: 'executor_missing', detail: null })),
          })),
        })) } });
      });
      await page.goto(`/topic/${ACME.fixLogin}`);
      const starter = page.locator('[data-quick-start]');
      await starter.getByRole('textbox').fill('Keep this first message');
      await starter.getByRole('button', { name: words.start.quick.change, exact: true }).click();
      const form = page.getByRole('dialog', { name: words.start.title.replace('{{parent}}', 'Fix login'), exact: true });
      await form.getByRole('combobox', { name: words.start.where.title, exact: true }).click();
      const judged = page.waitForResponse((response) => response.url().includes(`/nodes/${ACME.fixLogin}/binding/options`) && new URL(response.url()).searchParams.get('environment_new') === 'true');
      await page.getByRole('option', { name: words.choiceCatalog.environmentDestination.new.name, exact: true }).click();
      await judged;
      await expect(form.getByRole('button', { name: words.start.create, exact: true })).toBeDisabled();
      await form.getByRole('combobox', { name: words.start.agent.title, exact: true }).click();
      const profile = page.getByRole('option', { name: 'Codex', exact: true });
      await expect(profile).toHaveAttribute('aria-disabled', 'true');
      await expect(profile).toHaveAccessibleDescription(new RegExp(words.start.reason.executor_missing));
      await info.attach('unavailable-new-environment', { body: await page.screenshot(), contentType: 'image/png' });
      await page.keyboard.press('Escape');
      await form.getByRole('button', { name: words.start.options.title, exact: true }).click();
      await expect(form.getByRole('textbox', { name: words.start.message.title, exact: true })).toHaveValue('Keep this first message');
      await expectOrdinaryWizardControls(form, 'unavailable setup');
      expect(await requestsOf('POST', /\/(environments|threads|topics)$/)).toHaveLength(0);
      expect(await requestsOf('PUT', /\/default-environment$/)).toHaveLength(0);
    });

    test('a disabled Switch explains the wait beside ordinary action buttons', async ({ page }, info) => {
      await openHistory(page); await language(page, locale);
      const words = locale === 'en' ? english : chinese;
      await page.goto(`/thread/${MAIN}`);
      await page.getByRole('button', { name: words.change.menu.switch, exact: true }).click();
      const form = page.getByRole('dialog').filter({ has: page.locator('[data-thread-change-pickers]') });
      const confirm = form.getByRole('button', { name: words.change.confirm.switch, exact: true });
      await expect(confirm).toHaveAttribute('aria-disabled', 'true');
      await expect(form.getByRole('list', { name: words.change.steps.label })).toHaveCount(0);
      await expect(form.locator('[data-change-reason]')).toHaveText(words.changeReady.unchanged);
      await expectOrdinaryWizardControls(form, 'unchanged switch');
      await expectSameActionRow(confirm.locator('..'));
      let release: () => void = () => undefined;
      const held = new Promise<void>((resolve) => { release = resolve; });
      await page.route('**/api/v1/threads/*/forks/preflight', async (route) => { await held; await route.continue(); }, { times: 1 });
      try {
        const provider = form.getByRole('combobox', { name: words.switchChoice.picker, exact: true });
        const fromOpenAI = (await provider.textContent())?.includes('OpenAI key') === true;
        const [request] = await Promise.all([
          page.waitForRequest('**/api/v1/threads/*/forks/preflight'),
          (async () => { await provider.click(); await page.getByRole('option', { name: fromOpenAI ? /Team Anthropic/ : /OpenAI key/ }).first().click(); })(),
        ]);
        const body: unknown = (await bodySent(request));
        expect(body).toMatchObject({ to: { model: fromOpenAI ? 'claude-sonnet-5' : 'gpt-5.4' } });
        await expect(confirm).toHaveAttribute('aria-disabled', 'true');
        await expect(form.locator('[data-change-reason]')).toHaveText(words.changeReady.checking);
        await expectSameActionRow(confirm.locator('..'));
        await info.attach('switch-waiting-with-reason', { body: await page.screenshot(), contentType: 'image/png' });
        release();
        await expect(form.locator('[data-plan]')).toBeVisible();
        await expect(form.locator('[data-plan]')).not.toHaveAttribute('data-planning', 'true');
        expect(await form.locator('[data-change-reason]').allTextContents()).not.toContain(words.changeReady.checking);
        // A native policy refusal also has to be visible and associated; an allowed plan has no refusal.
        const refused = await confirm.getAttribute('aria-disabled') === 'true';
        expect(await confirm.getAttribute('aria-describedby')).toBe(refused ? await form.locator('[data-change-reason]').getAttribute('id') : null);
        if (refused) await expect(form.locator('[data-change-reason]')).toBeVisible();
        await expectOrdinaryWizardControls(form, 'reviewed switch');
        await info.attach('switch-reviewed', { body: await page.screenshot(), contentType: 'image/png' });
      } finally { release(); }
    });
  });
}
