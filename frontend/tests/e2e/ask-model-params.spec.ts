/** Ask's personal model parameters and configured-list policy, through their real screens in both languages. */
import { expect, test, type Page } from '@playwright/test';
import type { AskModelsReply, ProviderModel, ResolvedSetting, SettingDef } from '../../src/generated/api-types';
import { RUNS_ON } from '../doubles/fixtures';
import { askConversation, askModels, askSnapshot } from '../unit/ask-fixtures';
import { planProvider } from '../unit/provider-plan-fixtures';
import { fixture, language } from './ask-support';
import { ACME, scenario, supplyDefaults } from './support';

const schema = { type: 'object', additionalProperties: false, properties: {
  reasoning: { type: 'object', properties: { effort: { type: 'string', enum: ['low', 'high'], title: 'Effort' } } },
  temperature: { type: 'number', title: 'Temperature', minimum: 0, maximum: 1 },
} };
function choices(): AskModelsReply {
  const reply = askModels(), first = reply.models[0];
  if (first === undefined) throw new Error('The configured fixture needs a native entry');
  if (first.provider === null || first.model === null) throw new Error('The configured fixture needs native provider and model details');
  return { ...reply, models: [first, { ...first, is_default: false,
    selection: { provider_id: first.provider.id, model: 'ask-second' },
    request_option_schema: schema,
    model: { ...first.model, model_name: 'ask-second', display_name: 'Second API model',
      parameters: { efforts: ['low', 'high'], default_effort: 'low' }, option_constraints: { 'openai-responses': schema } },
  }, ...reply.models.slice(1)] };
}
function providerModels(reply: AskModelsReply): ProviderModel[] {
  return reply.models.flatMap((entry, index) => {
    if (entry.model === null) return [];
    const { allowed: _allowed, reason: _reason, ...model } = entry.model;
    return [{ ...model, groups: model.groups ?? [], id: `ask-model-${index}`, enabled: entry.allowed, source: 'manual' as const }];
  });
}
async function modelMetadata(page: Page, reply: AskModelsReply) {
  const provider = planProvider({ id: RUNS_ON.openAiKey, name: 'Team API', kind: 'api', protocols: ['openai-responses'] });
  await page.route((url) => url.pathname === `/api/v1/providers/${provider.id}`, (route) => route.fulfill({ json: provider }));
  await page.route((url) => url.pathname === `/api/v1/providers/${provider.id}/models`, (route) => route.fulfill({ json: providerModels(reply) }));
  return provider;
}

for (const locale of ['en', 'zh-CN'] as const) for (const theme of ['light', 'dark'] as const) {
  test(`Ask sends personal effort and another declared parameter (${locale}/${theme})`, async ({ page, request }, info) => {
    // EN: Topic > Ask > Default resolves to the first usable model > choose the second model's High effort > set Temperature > Send.
    // 中文：话题 > 问一问 > 默认显示首个可用模型 > 选择第二个模型的高推理强度 > 设置 Temperature > 发送。
    // The accepted body carries both personal parameters; the first question needs no manual model selection.
    // 请求同时携带两个个人参数；首次提问无需手动选择模型。
    await scenario(); const server = await fixture(page), reply = choices();
    const t = await language(page, request, locale, theme);
    const providerReads: string[] = []; let watching = false;
    await page.route((url) => url.pathname === `/api/v1/providers/${RUNS_ON.openAiKey}` || url.pathname === `/api/v1/providers/${RUNS_ON.openAiKey}/models`, (route) => {
      if (watching) providerReads.push(new URL(route.request().url()).pathname);
      return route.fulfill({ status: 403, json: { status: 403, code: 'authz.denied', message: 'Provider metadata is not readable', request_id: 'ask-model-only' } });
    });
    await page.route((url) => url.pathname === '/api/v1/ask/models', (route) => route.fulfill({ json: reply }));
    await page.goto(`/topic/${ACME.fixLogin}`);
    await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    const trigger = panel.getByRole('button', { name: t('ask.model'), exact: true });
    await expect(trigger).toContainText(`${t('ask.defaultModel')} · Readable API model`);
    const box = panel.getByRole('textbox', { name: t('ask.composer') });
    await expect(box).toBeEnabled();
    await page.screenshot({ path: info.outputPath('ask-model-before-choice.png'), animations: 'disabled' });
    watching = true;
    await trigger.click();
    const picker = page.getByRole('dialog', { name: t('ask.model'), exact: true });
    await expect(picker.getByRole('menuitemradio').first()).toHaveAccessibleName(t('ask.defaultModel'));
    await expect(picker.getByRole('menuitemradio', { name: 'Excluded API model', exact: true })).toHaveCount(0);
    await expect(picker.getByRole('group', { name: t('composer.model.efforts', { model: 'Readable API model' }) })).toHaveCount(0);
    await picker.getByRole('group', { name: t('composer.model.efforts', { model: 'Second API model' }) })
      .getByRole('menuitemradio', { name: t('choiceCatalog.effort.high.name'), exact: true }).click();
    const temperature = picker.getByRole('spinbutton', { name: 'Temperature' });
    await expect(temperature).toHaveAttribute('min', '0'); await expect(temperature).toHaveAttribute('max', '1');
    await expect(picker.getByRole('combobox', { name: 'Effort', exact: true })).toHaveCount(0);
    await temperature.fill('0.5');
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(temperature).toBeVisible();
      await expect.poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
      await page.screenshot({ path: info.outputPath(`ask-model-chosen-${width}.png`), animations: 'disabled' });
    }
    expect(providerReads).toEqual([]); watching = false;
    await page.keyboard.press('Escape');
    await box.fill('Read this organization using my selected parameters'); await box.press('Enter');
    await expect.poll(() => server.sends.length).toBe(1);
    expect(server.sends[0]?.model).toEqual({ provider_id: RUNS_ON.openAiKey, model: 'ask-second', params: { effort: 'high', temperature: 0.5 } });
  });

  for (const writable of [false, true]) test(`Ask blocks an empty configured list with the native writable hint ${writable} (${locale}/${theme})`, async ({ page, request }, info) => {
    // EN: Topic > Ask > read the blocked reason. With write authority, Configure Ask models opens the exact root policy; otherwise no link/input is offered.
    // 中文：话题 > 问一问 > 阅读阻止原因。有设置权限时，“配置问一问模型”打开该根组织的策略；否则不显示链接或输入框。
    await scenario(); const server = await fixture(page, [askSnapshot()]), t = await language(page, request, locale, theme);
    await page.route((url) => url.pathname === '/api/v1/ask/models', (route) => route.fulfill({ json: askModels(ACME.org, { models: [], default_model: null, blocked_reason: 'empty', writable }) }));
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(panel.getByText(t('ask.blockedEmpty'), { exact: true })).toBeVisible();
      await expect(panel.getByRole('textbox', { name: t('ask.composer') })).toHaveCount(0);
      await expect(panel.getByRole('button', { name: t('ask.retry'), exact: true })).toBeDisabled();
      await page.screenshot({ path: info.outputPath(`ask-model-blocked-${width}.png`), animations: 'disabled' });
    }
    const link = panel.getByRole('link', { name: t('ask.configureModels'), exact: true });
    if (writable) await expect(link).toHaveAttribute('href', '/org/acme/settings/defaults?setting=ask.models');
    else await expect(link).toHaveCount(0);
    expect(server.sends).toEqual([]); expect(server.creates).toEqual([]);
  });

  test(`Ask keeps the refused parameter reason and an independent running Stop (${locale}/${theme})`, async ({ page, request }, info) => {
    // EN: Topic > Ask > read the native refused-default-parameters reason > no composer > Stop the already running answer.
    // 中文：话题 > 问一问 > 阅读默认参数不可用的原因 > 不显示输入区 > 停止已经开始的回答。
    await scenario(); const server = await fixture(page, [askSnapshot({ conversation: askConversation({ state: 'running' }) })]);
    const t = await language(page, request, locale, theme), configured = askModels().models[0];
    if (configured === undefined) throw new Error('The configured entry is required.');
    await page.route((url) => url.pathname === '/api/v1/ask/models', (route) => route.fulfill({ json: askModels(ACME.org, {
      default_model: null, blocked_reason: 'no_usable_models', writable: false,
      models: [{ ...configured, allowed: false, is_default: false, unavailable_reason: 'parameters_unavailable' }],
    }) }));
    await page.goto(`/topic/${ACME.fixLogin}`); await page.getByRole('button', { name: t('ask.open'), exact: true }).click();
    const panel = page.locator('[data-view-window="ask"]');
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(panel.getByText(`${t('ask.blockedNoUsableModels')} · ${t('ask.modelExclusion_parameters_unavailable')}`, { exact: true })).toBeVisible();
      await expect(panel.getByRole('textbox', { name: t('ask.composer') })).toHaveCount(0);
      await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toBeEnabled();
      await page.screenshot({ path: info.outputPath(`ask-model-refusal-before-stop-${width}.png`), animations: 'disabled' });
    }
    await panel.getByRole('button', { name: t('composer.stop'), exact: true }).click();
    await expect.poll(() => server.stops).toBe(1);
    await expect(panel.getByRole('button', { name: t('composer.stop'), exact: true })).toHaveCount(0);
    expect(server.sends).toEqual([]);
    await page.screenshot({ path: info.outputPath('ask-model-refusal-after-stop.png'), animations: 'disabled' });
  });

  for (const system of [true, false]) test(`Ask models settings ${system ? 'system ordering' : 'org narrowing'} (${locale}/${theme})`, async ({ page, request }, info) => {
    // EN: Management > Settings > Automation > Ask models > move the second entry up > Ctrl+Enter; at Org Settings > Policies remove an inherited entry instead.
    // 中文：管理 > 设置 > 自动化 > 问一问模型 > 将第二项上移 > Ctrl+Enter；在组织设置 > 策略中只能移除继承项。
    // The saved list has identities only. An org has no Add, replacement or reorder control and no unrestricted catalogue request.
    // 保存的列表仅含模型身份；组织没有添加、替换或排序控件，也不会读取不受限制的候选列表。
    await scenario(); const t = await language(page, request, locale, theme);
    if (system) {
      // Use the product's existing switch, whose label includes the current translated state.
      const indicator = page.locator('[data-screen-titlebar]:visible').getByRole('switch');
      if (await indicator.getAttribute('data-as-admin') !== 'true') await indicator.click();
      await expect(indicator).toHaveAttribute('data-as-admin', 'true');
    }
    const reply = choices(), provider = await modelMetadata(page, reply);
    const initial = reply.models.filter((entry) => entry.allowed).map((entry) => entry.selection);
    const title = locale === 'en' ? 'Ask models' : '问一问模型列表';
    const def: SettingDef = { key: 'ask.models', title, description: title, class: 'policy', group: 'automation', scopes: ['system', 'org'],
      merge: 'intersect', default: null, example: null, text_key: null, keywords: [], schema: { type: 'array', uniqueItems: true,
        items: { type: 'object', required: ['provider_id', 'model'], additionalProperties: false, properties: { provider_id: { type: 'string' }, model: { type: 'string' } } } } };
    let current: ResolvedSetting = { key: 'ask.models', value: initial, own: system ? { value: initial, revision: 1, locked: false } : null,
      writable: true, source: { scope: 'system', node: null }, locked_by: null, parts: [] };
    const path = system ? '/api/v1/admin/settings' : `/api/v1/nodes/${ACME.org}/settings`;
    const writes: unknown[] = [], catalogues: string[] = [];
    await page.route((url) => url.pathname === '/api/v1/providers', (route) => { catalogues.push(route.request().url()); return route.fulfill({ json: { items: [{ ...provider, content_hidden: false }], next_cursor: null } }); });
    await page.route((url) => url.pathname === '/api/v1/settings/registry', (route) => route.fulfill({ json: [def] }));
    await page.route((url) => url.pathname === path || url.pathname.startsWith(`${path}/ask.models`), async (route) => {
      const request = route.request(), url = new URL(request.url());
      if (url.pathname.endsWith('/reach')) { await route.fulfill({ json: { applies: { orgs: 0, projects: 0, topics: 0, threads: 0 }, overrides: { items: [], next_cursor: null } } }); return; }
      if (request.method() === 'PUT') {
        const body = request.postDataJSON() as { value: unknown }; writes.push(body.value);
        current = { ...current, value: body.value, own: { value: body.value, revision: 2, locked: false } };
        await route.fulfill({ json: current }); return;
      }
      await route.fulfill({ json: url.pathname === path ? [current] : current });
    });
    if (!system) await supplyDefaults(page, ACME.org, { registry: () => [def], settings: () => [current] });
    await page.goto(system ? '/admin/settings?setting=ask.models' : '/org/acme/settings/defaults?setting=ask.models');
    const row = page.locator('[data-setting="ask.models"]'), list = row.getByRole('list', { name: title });
    await row.getByRole('button', { name: t('settings.compact.editValue', { name: title }), exact: true }).click();
    await expect(list.locator(':scope > li')).toHaveCount(2);
    const firstName = t('modelList.entry', { number: '1' }), secondName = t('modelList.entry', { number: '2' });
    for (const width of [1440, 400]) {
      await page.setViewportSize({ width, height: 900 });
      await expect(list).toBeVisible();
      await expect(row.getByRole('textbox', { name: /JSON/i })).toHaveCount(0);
      await page.screenshot({ path: info.outputPath(`ask-model-list-before-${width}.png`), animations: 'disabled' });
    }
    if (system) {
      await expect(row.getByRole('button', { name: t('modelList.add'), exact: true })).toBeEnabled();
      const up = row.getByRole('button', { name: t('modelList.moveUp', { model: secondName }), exact: true });
      await up.click(); await up.press('Control+Enter');
      await expect.poll(() => writes.length).toBe(1); expect(writes[0]).toEqual([initial[1], initial[0]]);
      expect(catalogues).not.toEqual([]);
    } else {
      await expect(row.getByText(t('ask.modelsFromAbove'), { exact: true })).toBeVisible();
      await expect(row.getByRole('button', { name: firstName, exact: true })).toBeDisabled();
      await expect(row.getByRole('button', { name: t('modelList.add'), exact: true })).toHaveCount(0);
      await expect(row.getByRole('button', { name: t('modelList.moveUp', { model: secondName }), exact: true })).toHaveCount(0);
      await row.getByRole('button', { name: t('modelList.remove', { model: firstName }), exact: true }).click();
      await expect(list.locator(':scope > li')).toHaveCount(1);
      await row.getByRole('button', { name: t('modelList.remove', { model: firstName }), exact: true }).press('Control+Enter');
      await expect.poll(() => writes.length).toBe(1); expect(writes[0]).toEqual([initial[1]]); expect(catalogues).toEqual([]);
    }
    await page.screenshot({ path: info.outputPath('ask-model-list-after-save.png'), animations: 'disabled' });
  });
}
