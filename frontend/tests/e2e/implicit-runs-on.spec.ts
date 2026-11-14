/** Actual runtime defaults explain their origin; rejected readable choices keep their native plan. */
import { expect, test, type Page } from '@playwright/test';
import type { TFunction } from 'i18next';
import type { CopyDestinationsRequest, CopyDestinationsResponse } from '../../src/api/copyDestinations';
import type { AgentListed, ForkRequest, ProviderModel } from '../../src/generated/api-types';
import { id, KIND } from '../doubles/fixtures';
import { language } from './ask-support';
import { ACME, scenario } from './support';

const subscription = id(KIND.provider, 2);
const keeper = id(KIND.agent, 4), scripted = id(KIND.agent, 3);
async function availability(page: Page) {
  // Keep real readable fixture identities, including the keeper's ordinary live admission.
  await page.route((url) => url.pathname === '/api/v1/agents', async (route) => {
    const response = await route.fetch(); const agents = await response.json() as AgentListed[];
    await route.fulfill({ response, json: agents.map((agent) => agent.content_hidden || agent.id !== scripted ? agent : { ...agent, disabled: true }) });
  });
  await page.route(`**/api/v1/providers/${subscription}/models`, async (route) => {
    const response = await route.fetch(); const models = await response.json() as ProviderModel[];
    await route.fulfill({ response, json: models.map((model) => ({ ...model, parameters: { ...model.parameters, efforts: ['medium'], default_effort: 'medium' } })) });
  });
  // The shared double has no destination judge yet. A thread destination requires its real fork plan,
  // rather than a synthetic permission grant; the unmodified preflight below still decides the proposal.
  await page.route('**/api/v1/copy-destinations/judge', async (route) => {
    const body = route.request().postDataJSON() as CopyDestinationsRequest;
    expect(body.subject).toEqual({ kind: 'thread', id: ACME.mainThread }); expect(body.intent).toBe('duplicate');
    const reply: CopyDestinationsResponse = { subject: body.subject, intent: body.intent, source_owned: true,
      destinations: body.destinations.map((destination) => ({ destination, cross_root: false, outcome: { state: 'needs_choice', choice: 'fork_plan' } })) };
    await route.fulfill({ json: reply });
  });
  const previews: ForkRequest[] = [];
  page.on('request', (request) => {
    if (request.method() === 'POST' && new URL(request.url()).pathname === `/api/v1/threads/${ACME.mainThread}/forks/preflight`) previews.push(request.postDataJSON() as ForkRequest);
  });
  const admitted: ForkRequest[] = [];
  page.on('response', (response) => {
    if (response.status() === 200 && new URL(response.url()).pathname === `/api/v1/threads/${ACME.mainThread}/forks/preflight`) admitted.push(response.request().postDataJSON() as ForkRequest);
  });
  return { previews, admitted };
}
async function openChange(page: Page, t: TFunction, kind: 'switch' | 'fork') {
  const name = t(kind === 'switch' ? 'change.menu.switch' : 'duplicate.to');
  await page.locator('[data-thread-more]').waitFor();
  const button = page.getByRole('button', { name, exact: true });
  if (await button.isVisible()) await button.click();
  else {
    await page.locator('[data-thread-more]').click();
    await page.getByRole('menuitem', { name: t('thread.header.placement'), exact: true }).hover();
    await page.getByRole('menuitem', { name, exact: true }).click();
  }
  return page.getByRole('dialog', { name: t(kind === 'switch' ? 'change.title.switch' : 'change.title.fork', { name: 'Main thread' }), exact: true });
}
for (const locale of ['en', 'zh-CN'] as const) for (const width of [400, 1440]) for (const theme of ['light', 'dark'] as const) {
  // Thread → Switch → inspect refused profiles with each provider → choose a profile → retained model/effort → disabled model → explicit different model → native preview → Cancel.
  // 会话 → 切换 → 按提供方查看不可用配置的原因 → 选择配置 → 沿用模型与推理强度 → 已停用模型 → 手动改选其他模型 → 原生预览 → 取消。
  test(`switch keeps refusals and explains retained choices ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const t = await language(page, request, locale, theme); const { previews, admitted } = await availability(page);
    await page.goto(`/thread/${ACME.mainThread}`); const form = await openChange(page, t, 'switch');
    const provider = form.getByRole('combobox', { name: t('change.runs.provider'), exact: true });
    const picker = form.getByRole('combobox', { name: t('change.runs.agent'), exact: true });
    await picker.click(); await picker.fill('Scripted agent');
    const disabled = page.getByRole('option', { name: 'Scripted agent', exact: true });
    await expect(disabled).toHaveAttribute('aria-disabled', 'true'); await expect(disabled).toContainText(t('change.runs.unavailable.profileDisabled'));
    await picker.fill('Claude Code');
    const unreported = page.getByRole('option', { name: 'Claude Code', exact: true });
    await expect(unreported).toHaveAttribute('aria-disabled', 'true'); await expect(unreported).toContainText(t('start.agent.unreported'));
    await picker.press('Escape');
    await provider.click(); await provider.fill('Codex subscription');
    await page.getByRole('option', { name: /Codex subscription/ }).first().click();
    await picker.click(); await picker.fill('Codex');
    const nonKeeper = page.getByRole('option', { name: 'Codex', exact: true });
    await expect(nonKeeper).toHaveAttribute('aria-disabled', 'true'); await expect(nonKeeper).toContainText(t('change.runs.unavailable.keeper'));
    await info.attach('switch-refused-profiles', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    await picker.press('Escape');
    await provider.click(); await provider.fill('OpenAI key');
    await page.getByRole('option', { name: /OpenAI key/ }).first().click();
    await picker.click(); await picker.fill('Codex on ada-laptop');
    await page.getByRole('option', { name: 'Codex on ada-laptop', exact: true }).click();
    await expect(form.locator('[data-automatic-runs="model"]')).toContainText(t('change.runs.automatic.model.retained'));
    await expect(form.locator('[data-automatic-runs="effort"]')).toContainText(t('change.runs.automatic.effort.retained'));
    await expect(form.locator('[data-automatic-runs="profile"]')).toHaveCount(0);
    await expect.poll(() => admitted.some((preview) => preview.result === 'replace' && preview.to?.agent_id === keeper && preview.to.model === 'gpt-5.4' && preview.to.effort === 'high')).toBe(true);
    await form.getByRole('combobox', { name: t('change.runs.model'), exact: true }).click();
    const refusedModel = page.getByRole('option', { name: 'GPT-4.1', exact: true });
    await expect(refusedModel).toHaveAttribute('aria-disabled', 'true'); await expect(refusedModel).toContainText(t('change.runs.unavailable.modelDisabled'));
    await page.getByRole('option', { name: 'GPT-5.4 Chat', exact: true }).click();
    await expect(form.locator('[data-automatic-runs="model"]')).toHaveCount(0);
    await expect.poll(() => admitted.some((preview) => preview.result === 'replace' && preview.to?.model === 'gpt-5.4-chat')).toBe(true);
    expect(previews.every((preview) => !Object.hasOwn(preview, 'automaticRuns') && !Object.hasOwn(preview.to ?? {}, 'automaticRuns'))).toBe(true);
    await info.attach('switch-explicit-model', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    await info.attach('native-switch-previews', { body: JSON.stringify({ requested: previews, admitted }), contentType: 'application/json' });
    await form.getByRole('button', { name: t('common.cancel'), exact: true }).click();
  });

  // Thread → Duplicate to → choose topic → subscription → automatic keeper/first-model/default-effort and old-choice reasons → disabled profiles → native preview → Cancel.
  // 会话 → 复制到 → 选择主题 → 订阅提供方 → 自动保管配置、首个模型与默认推理强度及原选择的原因 → 禁用配置 → 原生预览 → 取消。
  test(`fork names keeper and replacement origins ${locale}/${width}/${theme}`, async ({ page, request }, info) => {
    await scenario(); await page.setViewportSize({ width, height: 900 }); await page.emulateMedia({ colorScheme: theme });
    const t = await language(page, request, locale, theme); const { previews, admitted } = await availability(page);
    await page.goto(`/thread/${ACME.mainThread}`); const form = await openChange(page, t, 'fork');
    const destination = form.getByRole('combobox', { name: t('duplicate.destination'), exact: true });
    await destination.fill('Fix login'); await page.getByRole('option', { name: /Fix login/ }).click();
    const provider = form.getByRole('combobox', { name: t('change.runs.provider'), exact: true });
    await provider.click(); await provider.fill('Codex subscription'); await page.getByRole('option', { name: 'Codex subscription', exact: true }).click();
    await expect(form.locator('[data-automatic-runs="profile"]')).toContainText(t('change.runs.automatic.profile.keeper'));
    await expect(form.locator('[data-automatic-runs="profile"]')).toContainText(t('change.runs.automatic.previousProfile.keeper'));
    await expect(form.locator('[data-automatic-runs="model"]')).toContainText(t('change.runs.automatic.model.first'));
    await expect(form.locator('[data-automatic-runs="model"]')).toContainText(t('change.runs.automatic.previousModel'));
    await expect(form.locator('[data-automatic-runs="effort"]')).toContainText(t('change.runs.automatic.effort.default'));
    await expect(form.locator('[data-automatic-runs="effort"]')).toContainText(t('change.runs.automatic.previousEffort'));
    const frame = await form.boundingBox(); if (frame === null) throw new Error('The native change form is visible');
    for (const line of await form.locator('[data-automatic-runs]').all()) {
      await line.scrollIntoViewIfNeeded(); const bounds = await line.boundingBox(); if (bounds === null) throw new Error('The origin is visible beside its choice');
      expect(bounds.x).toBeGreaterThanOrEqual(frame.x); expect(bounds.x + bounds.width).toBeLessThanOrEqual(frame.x + frame.width + 1);
    }
    await info.attach('fork-automatic-origins', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    const profile = form.getByRole('combobox', { name: t('change.runs.agent'), exact: true }); await profile.click(); await profile.fill('Codex');
    const previous = page.getByRole('option', { name: 'Codex', exact: true });
    await expect(previous).toHaveAttribute('aria-disabled', 'true'); await expect(previous).toContainText(t('change.runs.unavailable.keeper'));
    await page.keyboard.press('Escape');
    await expect.poll(() => admitted.some((preview) => preview.result === 'new' && preview.topic_id === ACME.fixLogin && preview.to?.provider_id === subscription && preview.to.agent_id === keeper && preview.to.model === 'gpt-5.5-codex' && preview.to.effort === 'medium')).toBe(true);
    await expect(form.getByRole('combobox', { name: t('change.runs.model'), exact: true })).toContainText('GPT-5.5 Codex');
    expect(previews.every((preview) => !Object.hasOwn(preview, 'automaticRuns') && !Object.hasOwn(preview.to ?? {}, 'automaticRuns'))).toBe(true);
    await info.attach('native-fork-previews', { body: JSON.stringify({ requested: previews, admitted }), contentType: 'application/json' });
    await form.getByRole('button', { name: t('common.cancel'), exact: true }).click();
  });
}
