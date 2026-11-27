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
    // The agents that cannot be chosen stay in the agent list, off, with why.
    await form.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) }).click();
    const disabled = page.getByRole('menuitemradio', { name: /^Scripted agent/ });
    await expect(disabled).toBeDisabled(); await expect(disabled.locator('xpath=ancestor::li')).toContainText(t('composer.pick.agent.why.disabled'));
    const unreported = page.getByRole('menuitemradio', { name: /^Claude Code/ });
    await expect(unreported).toBeDisabled(); await expect(unreported.locator('xpath=ancestor::li')).toContainText(t('composer.pick.agent.why.unreported'));
    await info.attach('switch-refused-agents', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    await page.getByRole('menuitemradio', { name: 'Codex on ada-laptop', exact: true }).click();
    // A simple switch says only which agent went with the provider; the model and the effort are kept and go into the plan.
    await expect(form.locator('[data-automatic-runs="profile"]')).toHaveCount(0);
    await expect.poll(() => admitted.some((preview) => preview.result === 'replace' && preview.to?.agent_id === keeper && preview.to.model === 'gpt-5.4' && preview.to.effort === 'high')).toBe(true);
    await form.getByRole('button', { name: new RegExp(`^${t('composer.model.chip')}`) }).click();
    const refusedModel = page.getByRole('menuitemradio', { name: 'GPT-4.1', exact: true });
    await expect(refusedModel).toBeDisabled(); await expect(refusedModel.locator('xpath=ancestor::li')).toContainText(t('composer.model.reason.disabled', { provider: 'OpenAI key' }));
    await page.getByRole('menuitemradio', { name: 'GPT-5.4 Chat', exact: true }).click();
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
    // The provider is the model's: choosing a model of the Codex subscription chooses it, with its keeper agent.
    await form.getByRole('button', { name: new RegExp(`^${t('composer.model.chip')}`) }).click();
    await page.getByRole('menuitemradio', { name: 'GPT-5.5 Codex', exact: true }).click();
    await expect(form.locator('[data-automatic-runs="profile"]')).toContainText(t('change.runs.automatic.profile.keeper'));
    await expect(form.locator('[data-automatic-runs="profile"]')).toContainText(t('change.runs.automatic.previousProfile.keeper'));
    await expect(form.locator('[data-automatic-runs="effort"]')).toContainText(t('change.runs.automatic.effort.default'));
    await expect(form.locator('[data-automatic-runs="effort"]')).toContainText(t('change.runs.automatic.previousEffort'));
    const frame = await form.boundingBox(); if (frame === null) throw new Error('The native change form is visible');
    for (const line of await form.locator('[data-automatic-runs]').all()) {
      await line.scrollIntoViewIfNeeded(); const bounds = await line.boundingBox(); if (bounds === null) throw new Error('The origin is visible beside its choice');
      expect(bounds.x).toBeGreaterThanOrEqual(frame.x); expect(bounds.x + bounds.width).toBeLessThanOrEqual(frame.x + frame.width + 1);
    }
    await info.attach('fork-automatic-origins', { body: await page.screenshot({ animations: 'disabled' }), contentType: 'image/png' });
    // A provider that runs its keeper agent only has nothing to choose among.
    await expect(form.getByRole('button', { name: new RegExp(`^${t('composer.row.agentName')}`) })).toBeDisabled();
    await expect.poll(() => admitted.some((preview) => preview.result === 'new' && preview.topic_id === ACME.fixLogin && preview.to?.provider_id === subscription && preview.to.agent_id === keeper && preview.to.model === 'gpt-5.5-codex' && preview.to.effort === 'medium')).toBe(true);
    await expect(form.getByRole('button', { name: new RegExp(`^${t('composer.model.chip')}`) })).toContainText('GPT-5.5 Codex');
    expect(previews.every((preview) => !Object.hasOwn(preview, 'automaticRuns') && !Object.hasOwn(preview.to ?? {}, 'automaticRuns'))).toBe(true);
    await info.attach('native-fork-previews', { body: JSON.stringify({ requested: previews, admitted }), contentType: 'application/json' });
    await form.getByRole('button', { name: t('common.cancel'), exact: true }).click();
  });
}
